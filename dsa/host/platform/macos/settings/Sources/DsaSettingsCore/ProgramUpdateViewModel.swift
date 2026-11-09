import Foundation

/// An update of DSA itself while it runs: what it is doing, the version, and that the user
/// should leave DSA alone until it is done. The service reports the phase in its status; while
/// setup replaces the service there is no service to ask, and that gap reads as DSA restarting,
/// not as an error. Once the new service answers, the page says which version runs. The same
/// rules as the Windows app's view model of the same name.
@MainActor
public final class ProgramUpdateViewModel: ObservableObject {
    /// How long the service may stay away during an update before the page stops waiting for
    /// it and says so.
    public static let restartLimit: TimeInterval = 10 * 60

    private let service: DsaService
    private let text: TextSource
    private let now: () -> Date

    @Published public private(set) var phase: ProgramUpdatePhase = .none
    /// The version the update installs, or the one installed or that failed.
    @Published public private(set) var version = ""
    /// Why the request to install now was refused, in the user's language.
    @Published public private(set) var error: String?

    /// Set while the request to install now waits for its answer.
    private var asking = false
    /// When the service went away during the update, or nil while it answers.
    private var awaySince: Date?
    /// The service stayed away longer than `restartLimit`.
    private var notBack = false
    private var polling = false

    /// Called once the service answers again after it went away during an update, so the
    /// pages read the new version's settings.
    public var cameBack: (() -> Void)?

    public init(service: DsaService, text: TextSource, now: @escaping () -> Date = Date.init) {
        self.service = service
        self.text = text
        self.now = now
    }

    /// Whether an update is under way, so the page shows its progress and the user should not
    /// close DSA or start Android apps.
    public var inProgress: Bool {
        [.starting, .downloading, .stoppingAndroid, .installing, .restarting].contains(phase)
    }

    /// Whether the install-now button may start an update: not while one runs.
    public var canStartUpdate: Bool { !inProgress }
    public var isRestarting: Bool { phase == .restarting }
    public var isApplied: Bool { phase == .applied }
    public var isFailed: Bool { phase == .failed }

    /// What the update is doing now, in the user's language.
    public var phaseText: String {
        switch phase {
        case .starting: return text.get("ProgramUpdate_Starting")
        case .downloading: return text.format("ProgramUpdate_Downloading", version)
        case .stoppingAndroid: return text.format("ProgramUpdate_StoppingAndroid", version)
        case .installing: return text.format("ProgramUpdate_Installing", version)
        case .restarting: return text.get("ProgramUpdate_Restarting")
        default: return ""
        }
    }

    public var hasVersion: Bool { inProgress && !version.isEmpty }
    public var versionText: String { hasVersion ? text.format("ProgramUpdate_Version", version) : "" }
    /// The line that asks the user to leave DSA alone while the update runs.
    public var keepOpenText: String { text.get("ProgramUpdate_KeepOpen") }
    public var appliedText: String { isApplied ? text.format("ProgramUpdate_Applied", version) : "" }
    public var failedText: String {
        guard isFailed else {
            return ""
        }
        return notBack ? text.format("ProgramUpdate_NotBack", version) : text.format("ProgramUpdate_Failed", version)
    }

    private func set(_ phase: ProgramUpdatePhase, _ version: String? = nil) {
        if let version, !version.isEmpty {
            self.version = version
        }
        self.phase = phase
    }

    /// Asks the service to install the newest version now and shows the update from the moment
    /// it is asked. Returns the service's answer, or nil when it was refused, in which case
    /// `error` says why. An answer without a version means DSA is up to date.
    @discardableResult
    public func updateNow() async -> ProgramUpdateAnswer? {
        if inProgress {
            return nil
        }
        error = nil
        notBack = false
        awaySince = nil
        version = ""
        asking = true
        set(.starting)
        do {
            let answer = try await service.updateNow()
            asking = false
            guard let installing = answer.installing else {
                set(.none)
                return answer
            }
            // A poll may already have seen a later phase, or the service go away.
            if [.starting, .downloading, .stoppingAndroid].contains(phase) {
                set(.installing, installing)
            } else {
                set(phase, installing)
            }
            return answer
        } catch {
            asking = false
            // Setup may stop the service before its answer arrives: once setup runs, a lost
            // answer is the restart, not a refusal.
            if phase == .installing || phase == .restarting {
                goneAway()
                return nil
            }
            self.error = Format.problem(text, error)
            set(.none)
            return nil
        }
    }

    /// Reads where the update stands from the running service, as a timer does while the page
    /// is shown. Never starts the service: while setup has stopped it, starting it again would
    /// run the old version under setup.
    public func poll() async {
        if polling {
            return
        }
        polling = true
        defer { polling = false }
        let status: StatusInfo
        do {
            status = try await service.runningStatus()
        } catch {
            // A service that is away matters only while an update runs; otherwise the other
            // pages say it cannot be reached.
            if inProgress {
                goneAway()
            }
            return
        }
        take(status.programUpdate)
    }

    private func goneAway() {
        let since = awaySince ?? now()
        awaySince = since
        if now().timeIntervalSince(since) > ProgramUpdateViewModel.restartLimit {
            notBack = true
            set(.failed)
            return
        }
        set(.restarting)
    }

    /// Shows what the service said about the update.
    private func take(_ reported: ProgramUpdateProgress?) {
        if awaySince != nil {
            awaySince = nil
            notBack = false
            switch reported?.phase {
            case .some(let phase) where phase != .none:
                // The service that setup started says how it went, or a service started while
                // setup still runs waits for it.
                set(phase, reported?.version)
            default:
                // A new service with nothing to report runs the version that was installed.
                set(.applied)
            }
            cameBack?()
            return
        }
        if asking {
            // Until the request is answered, only a phase of this update counts: the result of
            // an earlier one, or nothing yet, leaves the page saying it is getting ready.
            if let reported, [.downloading, .stoppingAndroid, .installing, .restarting].contains(reported.phase) {
                set(reported.phase, reported.version)
            }
            return
        }
        guard let reported else {
            // Nothing is under way. An update the page followed keeps saying how it ended.
            if inProgress {
                set(.none)
            }
            return
        }
        set(reported.phase, reported.version)
    }
}
