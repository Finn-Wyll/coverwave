import SwiftUI
import UIKit

/// How touches on the art control playback. Swipes are the same in every
/// scheme (sideways: next/previous, up: library, down: queue, two-finger
/// tap: settings); the schemes differ in what a tap and a hold do.
enum GestureScheme: String, CaseIterable, Identifiable {
    /// Tap: play/pause. Hold and slide: scrub.
    case direct
    /// Tap the left, middle or right third: previous, play/pause, next.
    /// Hold and slide: scrub.
    case zones
    /// Tap: play/pause. Hold: a dial under the finger; slide to an option
    /// and let go.
    case dial

    var id: String { rawValue }

    var label: String {
        switch self {
        case .direct: "Direct"
        case .zones: "Zones"
        case .dial: "Dial"
        }
    }

    /// The cheat sheet, one gesture per line.
    var guide: [(gesture: String, action: String)] {
        var lines: [(String, String)] =
            switch self {
            case .direct:
                [("tap", "play / pause"), ("hold + slide", "scrub")]
            case .zones:
                [("tap left", "previous"), ("tap middle", "play / pause"), ("tap right", "next"),
                 ("hold + slide", "scrub")]
            case .dial:
                [("tap", "play / pause"), ("hold", "dial: slide, let go")]
            }
        lines += [
            ("swipe left", "next"), ("swipe right", "previous"),
            ("swipe up", "library"), ("swipe down", "queue"),
            ("two-finger tap", "settings"),
            ("two-finger swipe", "lyrics / cover"),
        ]
        return lines
    }
}

/// Raw touches on the art, from UIKit recognizers: SwiftUI has no
/// two-finger tap, and UIKit lets a hold and a pan compete cleanly (a pan
/// wins if the finger moves before the hold fires; after it fires the hold
/// tracks every movement).
enum SurfaceEvent {
    case tap(CGPoint)
    case twoFingerTap
    case twoFingerSwipe
    case panChanged(CGSize)
    case panEnded(translation: CGSize, velocity: CGSize)
    case holdBegan(CGPoint)
    case holdMoved(CGPoint)
    case holdEnded(CGPoint)
}

struct GestureSurface: UIViewRepresentable {
    var onEvent: (SurfaceEvent) -> Void

    func makeCoordinator() -> Coordinator { Coordinator(onEvent: onEvent) }

    func makeUIView(context: Context) -> UIView {
        let view = UIView()
        view.backgroundColor = .clear
        let c = context.coordinator

        let tap = UITapGestureRecognizer(target: c, action: #selector(Coordinator.tap(_:)))
        let two = UITapGestureRecognizer(target: c, action: #selector(Coordinator.twoFingerTap(_:)))
        two.numberOfTouchesRequired = 2
        let pan = UIPanGestureRecognizer(target: c, action: #selector(Coordinator.pan(_:)))
        pan.maximumNumberOfTouches = 1
        let hold = UILongPressGestureRecognizer(target: c, action: #selector(Coordinator.hold(_:)))
        hold.minimumPressDuration = 0.32
        hold.allowableMovement = 10
        let swipe = UISwipeGestureRecognizer(target: c, action: #selector(Coordinator.twoFingerSwipe(_:)))
        swipe.numberOfTouchesRequired = 2
        swipe.direction = [.left, .right]
        for r in [tap, two, pan, hold, swipe] { view.addGestureRecognizer(r) }
        return view
    }

    func updateUIView(_ view: UIView, context: Context) {
        context.coordinator.onEvent = onEvent
    }

    final class Coordinator: NSObject {
        var onEvent: (SurfaceEvent) -> Void
        init(onEvent: @escaping (SurfaceEvent) -> Void) { self.onEvent = onEvent }

        @objc func tap(_ r: UITapGestureRecognizer) {
            onEvent(.tap(r.location(in: r.view)))
        }

        @objc func twoFingerTap(_ r: UITapGestureRecognizer) {
            onEvent(.twoFingerTap)
        }

        @objc func twoFingerSwipe(_ r: UISwipeGestureRecognizer) {
            onEvent(.twoFingerSwipe)
        }

        @objc func pan(_ r: UIPanGestureRecognizer) {
            let t = r.translation(in: r.view)
            switch r.state {
            case .changed:
                onEvent(.panChanged(CGSize(width: t.x, height: t.y)))
            case .ended, .cancelled, .failed:
                let v = r.velocity(in: r.view)
                // A cancelled pan commits nothing: report no movement.
                let done = r.state == .ended
                onEvent(
                    .panEnded(
                        translation: done ? CGSize(width: t.x, height: t.y) : .zero,
                        velocity: done ? CGSize(width: v.x, height: v.y) : .zero))
            default: break
            }
        }

        @objc func hold(_ r: UILongPressGestureRecognizer) {
            let p = r.location(in: r.view)
            switch r.state {
            case .began: onEvent(.holdBegan(p))
            case .changed: onEvent(.holdMoved(p))
            case .ended, .cancelled, .failed: onEvent(.holdEnded(p))
            default: break
            }
        }
    }
}

/// Where a flick would come to rest (UIScrollView's deceleration curve).
func projected(_ translation: CGFloat, velocity: CGFloat, rate: CGFloat = 0.99) -> CGFloat {
    translation + (velocity / 1000) * rate / (1 - rate)
}

/// Past a boundary, follow less and less.
func rubberband(_ x: CGFloat, limit: CGFloat, c: CGFloat = 0.55) -> CGFloat {
    let sign: CGFloat = x < 0 ? -1 : 1
    let a = abs(x)
    return sign * (a * limit * c) / (limit + c * a)
}
