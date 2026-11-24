//! macOS 14.2+ Core Audio process tap: captures only the target apps' audio
//! (no Screen Recording permission), the way Apple's "Capturing system
//! audio with Core Audio taps" sample does.
//!
//! 1. Find the Core Audio process objects whose bundle ID starts with a
//!    target ID (`kAudioHardwarePropertyProcessObjectList`).
//! 2. Create a private, unmuted stereo-mixdown tap of those processes.
//! 3. Wrap the tap in a private aggregate device (main sub-device: the
//!    default output, so the clock matches what you hear).
//! 4. Read the aggregate device's input with an IO block and push a mono mix
//!    into the ring buffer.
//!
//! The first start asks for the "audio recording" permission. For the
//! prompt to show, the binary needs `NSAudioCaptureUsageDescription` in an
//! embedded Info.plist (`frontends/cw-tui/build.rs` does that).
//!
//! UNTESTED: written and type-checked in a Linux cloud session. See
//! LOCAL_TODO.md.

use crate::{CaptureError, Target};
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{AnyThread, Message};
use objc2_core_audio::{
    AudioDeviceCreateIOProcIDWithBlock, AudioDeviceDestroyIOProcID, AudioDeviceIOProcID,
    AudioDeviceStart, AudioDeviceStop, AudioHardwareCreateAggregateDevice,
    AudioHardwareCreateProcessTap, AudioHardwareDestroyAggregateDevice,
    AudioHardwareDestroyProcessTap, AudioObjectGetPropertyData, AudioObjectGetPropertyDataSize,
    AudioObjectID, AudioObjectPropertyAddress, AudioObjectPropertySelector, CATapDescription,
    CATapMuteBehavior, kAudioAggregateDeviceIsPrivateKey, kAudioAggregateDeviceIsStackedKey,
    kAudioAggregateDeviceMainSubDeviceKey, kAudioAggregateDeviceNameKey,
    kAudioAggregateDeviceSubDeviceListKey, kAudioAggregateDeviceTapAutoStartKey,
    kAudioAggregateDeviceTapListKey, kAudioAggregateDeviceUIDKey, kAudioDevicePropertyDeviceUID,
    kAudioHardwarePropertyDefaultOutputDevice, kAudioHardwarePropertyProcessObjectList,
    kAudioObjectPropertyElementMain, kAudioObjectPropertyScopeGlobal, kAudioObjectSystemObject,
    kAudioProcessPropertyBundleID, kAudioSubDeviceUIDKey, kAudioSubTapDriftCompensationKey,
    kAudioSubTapUIDKey, kAudioTapPropertyFormat,
};
use objc2_core_audio_types::{
    AudioBufferList, AudioStreamBasicDescription, AudioTimeStamp, kAudioFormatFlagIsFloat,
    kAudioFormatFlagIsNonInterleaved,
};
use objc2_core_foundation::CFDictionary;
use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSString};
use rtrb::Producer;
use std::cell::RefCell;
use std::ffi::{CStr, c_void};
use std::ptr::NonNull;

type IoBlock = RcBlock<
    dyn Fn(
        NonNull<AudioTimeStamp>,
        NonNull<AudioBufferList>,
        NonNull<AudioTimeStamp>,
        NonNull<AudioBufferList>,
        NonNull<AudioTimeStamp>,
    ),
>;

pub struct Tap {
    tap: AudioObjectID,
    device: AudioObjectID,
    proc_id: AudioDeviceIOProcID,
    sample_rate: u32,
    /// Keeps the IO block alive while the device runs.
    _block: IoBlock,
}

fn fail(what: &str, status: i32) -> CaptureError {
    CaptureError::Failed(format!("{what} (OSStatus {status})"))
}

fn address(selector: AudioObjectPropertySelector) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain,
    }
}

/// Reads a fixed-size property. `T` must be a plain C type (all-zero is a
/// valid value).
unsafe fn get<T: Copy>(
    object: AudioObjectID,
    selector: AudioObjectPropertySelector,
) -> Result<T, i32> {
    let addr = address(selector);
    let mut value: T = unsafe { std::mem::zeroed() };
    let mut size = std::mem::size_of::<T>() as u32;
    let status = unsafe {
        AudioObjectGetPropertyData(
            object,
            NonNull::from(&addr),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
            NonNull::from(&mut value).cast(),
        )
    };
    if status == 0 { Ok(value) } else { Err(status) }
}

/// Reads a property that is an array of `u32` object IDs.
unsafe fn get_ids(
    object: AudioObjectID,
    selector: AudioObjectPropertySelector,
) -> Result<Vec<AudioObjectID>, i32> {
    let addr = address(selector);
    let mut size = 0u32;
    let status = unsafe {
        AudioObjectGetPropertyDataSize(
            object,
            NonNull::from(&addr),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
        )
    };
    if status != 0 {
        return Err(status);
    }
    let mut ids = vec![0 as AudioObjectID; size as usize / std::mem::size_of::<AudioObjectID>()];
    if ids.is_empty() {
        return Ok(ids);
    }
    let status = unsafe {
        AudioObjectGetPropertyData(
            object,
            NonNull::from(&addr),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
            NonNull::new(ids.as_mut_ptr().cast::<c_void>()).expect("non-empty vec"),
        )
    };
    if status == 0 { Ok(ids) } else { Err(status) }
}

/// Reads a CFString property (returned +1 retained) as a Rust string.
unsafe fn get_string(
    object: AudioObjectID,
    selector: AudioObjectPropertySelector,
) -> Option<String> {
    let raw: usize = unsafe { get::<usize>(object, selector) }.ok()?;
    if raw == 0 {
        return None;
    }
    // CFString is toll-free bridged to NSString; take ownership of the +1.
    let s: Retained<NSString> = unsafe { Retained::from_raw(raw as *mut NSString) }?;
    Some(s.to_string())
}

fn ns(key: &CStr) -> Retained<NSString> {
    NSString::from_str(key.to_str().unwrap_or_default())
}

fn any<T: Message>(o: &T) -> &AnyObject {
    // Every Objective-C object is an AnyObject.
    unsafe { &*(o as *const T as *const AnyObject) }
}

fn dict(pairs: &[(&CStr, &AnyObject)]) -> Retained<NSDictionary<NSString, AnyObject>> {
    let keys: Vec<Retained<NSString>> = pairs.iter().map(|(k, _)| ns(k)).collect();
    let key_refs: Vec<&NSString> = keys.iter().map(|k| &**k).collect();
    let values: Vec<&AnyObject> = pairs.iter().map(|(_, v)| *v).collect();
    NSDictionary::from_slices(&key_refs, &values)
}

impl Tap {
    pub fn start(target: &Target, sink: Producer<f32>) -> Result<Tap, CaptureError> {
        unsafe { Self::start_inner(target, sink) }
    }

    unsafe fn start_inner(target: &Target, sink: Producer<f32>) -> Result<Tap, CaptureError> {
        let system = kAudioObjectSystemObject as AudioObjectID;

        // 1. The processes to tap.
        let description = match target {
            Target::System => {
                let none: Retained<NSArray<NSNumber>> = NSArray::from_slice(&[]);
                unsafe {
                    CATapDescription::initStereoGlobalTapButExcludeProcesses(
                        CATapDescription::alloc(),
                        &none,
                    )
                }
            }
            Target::Apps(ids) => {
                let procs = unsafe { get_ids(system, kAudioHardwarePropertyProcessObjectList) }
                    .map_err(|s| fail("listing audio processes", s))?;
                let matching: Vec<Retained<NSNumber>> = procs
                    .into_iter()
                    .filter(|&p| {
                        unsafe { get_string(p, kAudioProcessPropertyBundleID) }
                            .is_some_and(|b| ids.iter().any(|id| b.starts_with(id.as_str())))
                    })
                    .map(NSNumber::numberWithUnsignedInt)
                    .collect();
                if matching.is_empty() {
                    return Err(CaptureError::NotPlaying(ids.join(", ")));
                }
                let array = NSArray::from_retained_slice(&matching);
                unsafe {
                    CATapDescription::initStereoMixdownOfProcesses(
                        CATapDescription::alloc(),
                        &array,
                    )
                }
            }
        };
        unsafe {
            description.setName(&NSString::from_str("coverwave"));
            description.setPrivate(true);
            description.setMuteBehavior(CATapMuteBehavior::Unmuted);
        }

        // 2. The tap.
        let mut tap: AudioObjectID = 0;
        let status = unsafe { AudioHardwareCreateProcessTap(Some(&description), &mut tap) };
        if status != 0 {
            return Err(CaptureError::Denied(format!(
                "creating the process tap failed (OSStatus {status}); allow coverwave under \
                 System Settings > Privacy & Security > Screen & System Audio Recording"
            )));
        }
        let tap_uid = unsafe { description.UUID().UUIDString() };
        let format: AudioStreamBasicDescription = match unsafe { get(tap, kAudioTapPropertyFormat) }
        {
            Ok(f) => f,
            Err(s) => {
                unsafe { AudioHardwareDestroyProcessTap(tap) };
                return Err(fail("reading the tap format", s));
            }
        };

        // 3. A private aggregate device around it, clocked by the default output.
        let output: AudioObjectID =
            unsafe { get(system, kAudioHardwarePropertyDefaultOutputDevice) }.unwrap_or(0);
        let output_uid =
            unsafe { get_string(output, kAudioDevicePropertyDeviceUID) }.unwrap_or_default();
        let output_uid = NSString::from_str(&output_uid);
        let agg_uid = NSString::from_str(&format!("coverwave-tap-{}", std::process::id()));
        let name = NSString::from_str("coverwave capture");
        let yes = NSNumber::numberWithBool(true);
        let no = NSNumber::numberWithBool(false);
        let sub_device = dict(&[(kAudioSubDeviceUIDKey, any(&*output_uid))]);
        let sub_devices = NSArray::from_retained_slice(&[sub_device]);
        let sub_tap = dict(&[
            (kAudioSubTapUIDKey, any(&*tap_uid)),
            (kAudioSubTapDriftCompensationKey, any(&*yes)),
        ]);
        let sub_taps = NSArray::from_retained_slice(&[sub_tap]);
        let description = dict(&[
            (kAudioAggregateDeviceNameKey, any(&*name)),
            (kAudioAggregateDeviceUIDKey, any(&*agg_uid)),
            (kAudioAggregateDeviceMainSubDeviceKey, any(&*output_uid)),
            (kAudioAggregateDeviceIsPrivateKey, any(&*yes)),
            (kAudioAggregateDeviceIsStackedKey, any(&*no)),
            (kAudioAggregateDeviceTapAutoStartKey, any(&*yes)),
            (kAudioAggregateDeviceSubDeviceListKey, any(&*sub_devices)),
            (kAudioAggregateDeviceTapListKey, any(&*sub_taps)),
        ]);
        // NSDictionary is toll-free bridged to CFDictionary.
        let cf: &CFDictionary =
            unsafe { &*(Retained::as_ptr(&description) as *const CFDictionary) };
        let mut device: AudioObjectID = 0;
        let status = unsafe { AudioHardwareCreateAggregateDevice(cf, NonNull::from(&mut device)) };
        if status != 0 {
            unsafe { AudioHardwareDestroyProcessTap(tap) };
            return Err(fail("creating the aggregate device", status));
        }

        // 4. Read it.
        let float = format.mFormatFlags & kAudioFormatFlagIsFloat != 0;
        let non_interleaved = format.mFormatFlags & kAudioFormatFlagIsNonInterleaved != 0;
        if !float {
            unsafe {
                AudioHardwareDestroyAggregateDevice(device);
                AudioHardwareDestroyProcessTap(tap);
            }
            return Err(CaptureError::Unsupported("tap format is not float".into()));
        }
        let sink = RefCell::new(sink);
        let block: IoBlock = RcBlock::new(
            move |_now: NonNull<AudioTimeStamp>,
                  input: NonNull<AudioBufferList>,
                  _in_time: NonNull<AudioTimeStamp>,
                  _output: NonNull<AudioBufferList>,
                  _out_time: NonNull<AudioTimeStamp>| {
                // Real-time thread: no allocation, no locks (RefCell is a flag).
                let Ok(mut sink) = sink.try_borrow_mut() else {
                    return;
                };
                unsafe { push_buffers(&mut sink, input, non_interleaved) };
            },
        );
        let mut proc_id: AudioDeviceIOProcID = None;
        let status = unsafe {
            AudioDeviceCreateIOProcIDWithBlock(
                NonNull::from(&mut proc_id),
                device,
                None,
                RcBlock::as_ptr(&block),
            )
        };
        if status != 0 {
            unsafe {
                AudioHardwareDestroyAggregateDevice(device);
                AudioHardwareDestroyProcessTap(tap);
            }
            return Err(fail("registering the capture callback", status));
        }
        let status = unsafe { AudioDeviceStart(device, proc_id) };
        if status != 0 {
            unsafe {
                AudioDeviceDestroyIOProcID(device, proc_id);
                AudioHardwareDestroyAggregateDevice(device);
                AudioHardwareDestroyProcessTap(tap);
            }
            return Err(fail("starting capture", status));
        }
        Ok(Tap {
            tap,
            device,
            proc_id,
            sample_rate: format.mSampleRate.round() as u32,
            _block: block,
        })
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
}

/// Mixes an input buffer list (interleaved, or one buffer per channel) to
/// mono and pushes it. Real-time safe.
unsafe fn push_buffers(
    sink: &mut Producer<f32>,
    list: NonNull<AudioBufferList>,
    non_interleaved: bool,
) {
    let list = unsafe { list.as_ref() };
    let n = list.mNumberBuffers as usize;
    if n == 0 {
        return;
    }
    let buffers = unsafe { std::slice::from_raw_parts(list.mBuffers.as_ptr(), n) };
    let as_f32 = |i: usize| -> &[f32] {
        let b = &buffers[i];
        if b.mData.is_null() {
            return &[];
        }
        unsafe { std::slice::from_raw_parts(b.mData as *const f32, b.mDataByteSize as usize / 4) }
    };
    if non_interleaved && n > 1 {
        let frames = (0..n).map(|i| as_f32(i).len()).min().unwrap_or(0);
        let scale = 1.0 / n as f32;
        for f in 0..frames {
            let m: f32 = (0..n).map(|i| as_f32(i)[f]).sum::<f32>() * scale;
            if sink.push(m).is_err() {
                return;
            }
        }
    } else {
        let channels = buffers[0].mNumberChannels.max(1) as usize;
        crate::push_mono(sink, as_f32(0), channels);
    }
}

impl Drop for Tap {
    fn drop(&mut self) {
        unsafe {
            AudioDeviceStop(self.device, self.proc_id);
            AudioDeviceDestroyIOProcID(self.device, self.proc_id);
            AudioHardwareDestroyAggregateDevice(self.device);
            AudioHardwareDestroyProcessTap(self.tap);
        }
    }
}
