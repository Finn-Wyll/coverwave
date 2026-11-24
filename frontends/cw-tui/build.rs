//! On macOS, embeds `macos/Info.plist` in the binary (`__TEXT,__info_plist`)
//! so the audio-capture and Automation permission prompts can explain
//! themselves. Without `NSAudioCaptureUsageDescription`, process taps
//! deliver silence.

fn main() {
    println!("cargo:rerun-if-changed=macos/Info.plist");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        let dir = std::env::var("CARGO_MANIFEST_DIR").expect("set by cargo");
        println!(
            "cargo:rustc-link-arg-bins=-Wl,-sectcreate,__TEXT,__info_plist,{dir}/macos/Info.plist"
        );
    }
}
