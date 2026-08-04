import AppKit
import Darwin
import Foundation
import Observation
import QuartzCore

/// A sample set with the two numbers that decide anything: the middle and the tail. p50 says how
/// it feels, p95 says whether it ever stutters, and `poc/RESULTS.md` reports both for the same
/// reason.
struct Samples {
    private(set) var values: [Double] = []

    mutating func add(_ v: Double) { values.append(v) }

    var count: Int { values.count }
    var p50: Double { percentile(0.50) }
    var p95: Double { percentile(0.95) }
    var max: Double { values.max() ?? 0 }
    var mean: Double { values.isEmpty ? 0 : values.reduce(0, +) / Double(values.count) }

    /// Nearest-rank. With 20–40 samples an interpolating percentile invents precision the sample
    /// size does not have.
    func percentile(_ q: Double) -> Double {
        guard !values.isEmpty else { return 0 }
        let sorted = values.sorted()
        let rank = Swift.max(0, Swift.min(sorted.count - 1, Int((q * Double(sorted.count)).rounded(.up)) - 1))
        return sorted[rank]
    }

    var line: String {
        String(format: "p50 %6.1f  p95 %6.1f  max %6.1f  (n=%d)", p50, p95, max, count)
    }
}

/// Resident footprint of this process, in MB.
///
/// `phys_footprint` and not `resident_size`: it is the number macOS itself bills a process for,
/// and it counts the compressed pages a big decoded result set spends most of its life in.
func memoryFootprintMB() -> Double {
    var info = task_vm_info_data_t()
    var count = mach_msg_type_number_t(MemoryLayout<task_vm_info_data_t>.size / MemoryLayout<natural_t>.size)
    let kr = withUnsafeMutablePointer(to: &info) {
        $0.withMemoryRebound(to: integer_t.self, capacity: Int(count)) {
            task_info(mach_task_self_, task_flavor_t(TASK_VM_INFO), $0, &count)
        }
    }
    guard kr == KERN_SUCCESS else { return -1 }
    return Double(info.phys_footprint) / 1024 / 1024
}

/// How late the main thread was to each vsync.
///
/// The display link fires on the main run loop at every refresh whether or not anything is drawn,
/// so the *interval* between fires says nothing — it is flat at 1/refresh even while the
/// interface is frozen. What jank actually is: the main thread was busy, so the callback for a
/// vsync arrives long after that vsync happened. `lag = now − link.timestamp` is that, directly,
/// and it is the number a user is describing when they say an interface stutters.
@MainActor
@Observable
final class FrameRecorder {
    private var link: CADisplayLink?
    private(set) var lag = Samples()
    /// Read off the link rather than assumed: a hard-coded 60 would hide every dropped frame on a
    /// 120 Hz display, which is what this machine has.
    private(set) var refreshHz: Double = 60
    /// Set by the caller when it assigns data; cleared at the first vsync the main thread is free
    /// to service, which is the first moment that data could have been on screen.
    var pendingSince: Double?
    private(set) var toFrame = Samples()

    func attach(to view: NSView) {
        let link = view.displayLink(target: self, selector: #selector(tick(_:)))
        link.add(to: .main, forMode: .common)
        self.link = link
    }

    @objc private func tick(_ link: CADisplayLink) {
        let now = CACurrentMediaTime()
        let period = link.targetTimestamp - link.timestamp
        if period > 0 { refreshHz = 1 / period }
        lag.add((now - link.timestamp) * 1000)
        if let since = pendingSince {
            toFrame.add((now - since) * 1000)
            pendingSince = nil
        }
    }

    func reset() {
        lag = Samples()
        toFrame = Samples()
    }

    func stop() {
        link?.invalidate()
        link = nil
    }

    /// Vsyncs the main thread was more than one refresh period late for. Below that it made the
    /// frame; above it, something was skipped.
    var missed: Int {
        let period = 1000 / refreshHz
        return lag.values.filter { $0 > period }.count
    }
}

/// One line of a measurement table, so every bench prints in the same shape and RESULTS.md can be
/// pasted out of the terminal without anyone retyping a number.
func row(_ label: String, _ s: Samples) -> String {
    "  " + label.padding(toLength: 26, withPad: " ", startingAt: 0) + " " + s.line
}

/// Printed with every bench. This is a developer laptop with a browser and several agents on it,
/// not a quiet benchmark rig, and a number taken at load 15 means something different from the
/// same number taken at load 1 — so the reading is recorded next to the result rather than
/// remembered.
func machineLine() -> String {
    var loads = [Double](repeating: 0, count: 3)
    let n = getloadavg(&loads, 3)
    let cores = ProcessInfo.processInfo.activeProcessorCount
    let load = n > 0 ? loads[0...].prefix(Int(n)).map { String(format: "%.1f", $0) }.joined(separator: " ") : "?"
    return "machine: \(cores) cores, load \(load), \(fmt(memoryFootprintMB())) MB in this process"
}
