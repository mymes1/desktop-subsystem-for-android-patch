import XCTest
@testable import DsaSettingsCore

/// A dsa that answers each command line with the answers the test queued for it, in order; the
/// last one stays. The update request can be held until the test lets it go.
final class SequenceCli: DsaCli {
    private var answers: [String: [CliResult]] = [:]
    private(set) var calls: [String] = []
    /// Set to hold `update --json` until `release()`.
    var holdUpdate = false
    private var held: CheckedContinuation<Void, Never>?

    func then(_ line: String, _ json: String) {
        answers[line, default: []].append(CliResult(exitCode: 0, output: json, error: ""))
    }

    func thenFail(_ line: String, _ error: String) {
        answers[line, default: []].append(CliResult(exitCode: 1, output: "", error: error))
    }

    func release() {
        held?.resume()
        held = nil
    }

    var isHolding: Bool { held != nil }

    func run(_ args: [String]) async throws -> CliResult {
        let line = args.joined(separator: " ")
        calls.append(line)
        if line == "update --json" && holdUpdate {
            await withCheckedContinuation { held = $0 }
        }
        guard var queue = answers[line], !queue.isEmpty else {
            return CliResult(exitCode: 2, output: "", error: "dsa: the test has no answer for \(line)")
        }
        let answer = queue.count > 1 ? queue.removeFirst() : queue[0]
        answers[line] = queue
        return answer
    }
}

/// Gives each text as its key, with the version after it where the text takes one.
final class KeyAndVersionText: TextSource {
    func get(_ key: String) -> String { key + " {0}" }
}

@MainActor
final class ProgramUpdateTests: XCTestCase {
    private let status = "status --no-start --json"
    private let update = "update --json"
    private let unreachable = "dsa: cannot reach the DSA service (no such socket); check that it is installed and running"
    private let installing = #"{"installing":"0.1.11","detail":"installing DSA 0.1.11","status":null}"#

    private func statusWith(_ phase: String?, _ version: String = "0.1.11") -> String {
        let update = phase.map { #"{"phase":"\#($0)","version":"\#(version)","in_progress":true}"# } ?? "null"
        return #"{"state":"Stopped","last_reason":null,"update_notice":null,"problem":null,"program_update":\#(update)}"#
    }

    private func model(_ cli: SequenceCli, now: @escaping () -> Date = Date.init) -> ProgramUpdateViewModel {
        ProgramUpdateViewModel(service: DsaService(cli: cli), text: KeyAndVersionText(), now: now)
    }

    /// Starts the update request and waits until dsa holds it.
    private func startHeld(_ vm: ProgramUpdateViewModel, _ cli: SequenceCli) async -> Task<ProgramUpdateAnswer?, Never> {
        cli.holdUpdate = true
        let asking = Task { await vm.updateNow() }
        for _ in 0..<1000 where !cli.isHolding {
            await Task.yield()
        }
        XCTAssertTrue(cli.isHolding)
        return asking
    }

    func test_fr15_update_now_shows_progress_at_once_and_turns_the_button_off() async {
        let cli = SequenceCli()
        cli.then(update, installing)
        cli.then(status, statusWith(nil))
        cli.then(status, statusWith("failed", "0.1.10"))
        let vm = model(cli)
        XCTAssertTrue(vm.canStartUpdate)
        let asking = await startHeld(vm, cli)
        XCTAssertEqual(vm.phase, .starting)
        XCTAssertTrue(vm.inProgress)
        XCTAssertFalse(vm.canStartUpdate)
        XCTAssertEqual(vm.phaseText, "ProgramUpdate_Starting {0}")
        XCTAssertEqual(vm.keepOpenText, "ProgramUpdate_KeepOpen {0}")
        XCTAssertFalse(vm.hasVersion)
        // Before the service reports this update, nothing else it says moves the page.
        await vm.poll()
        XCTAssertEqual(vm.phase, .starting)
        await vm.poll()
        XCTAssertEqual(vm.phase, .starting)
        cli.release()
        let answer = await asking.value
        XCTAssertEqual(answer?.installing, "0.1.11")
        XCTAssertEqual(vm.phase, .installing)
        XCTAssertEqual(vm.phaseText, "ProgramUpdate_Installing 0.1.11")
    }

    func test_fr15_each_phase_the_service_reports_is_shown_with_its_version() async {
        let phases: [(String, ProgramUpdatePhase, String)] = [
            ("downloading", .downloading, "ProgramUpdate_Downloading 0.1.11"),
            ("stopping_android", .stoppingAndroid, "ProgramUpdate_StoppingAndroid 0.1.11"),
            ("installing", .installing, "ProgramUpdate_Installing 0.1.11"),
            ("restarting", .restarting, "ProgramUpdate_Restarting {0}"),
        ]
        for (name, phase, said) in phases {
            let cli = SequenceCli()
            cli.then(update, installing)
            cli.then(status, statusWith(name))
            let vm = model(cli)
            let asking = await startHeld(vm, cli)
            await vm.poll()
            XCTAssertEqual(vm.phase, phase, name)
            XCTAssertEqual(vm.phaseText, said, name)
            XCTAssertEqual(vm.versionText, "ProgramUpdate_Version 0.1.11", name)
            XCTAssertFalse(vm.canStartUpdate, name)
            cli.release()
            _ = await asking.value
            XCTAssertTrue(vm.inProgress, name)
        }
    }

    func test_fr15_the_restart_gap_reads_as_restarting_and_the_new_version_is_shown_once_it_answers() async {
        let cli = SequenceCli()
        cli.then(update, installing)
        cli.thenFail(status, unreachable)
        cli.thenFail(status, "dsa: the socket was closed")
        cli.then(status, statusWith("applied"))
        let vm = model(cli)
        var cameBack = 0
        vm.cameBack = { cameBack += 1 }
        await vm.updateNow()
        XCTAssertEqual(vm.phase, .installing)
        await vm.poll()
        XCTAssertEqual(vm.phase, .restarting)
        XCTAssertEqual(vm.phaseText, "ProgramUpdate_Restarting {0}")
        XCTAssertNil(vm.error, "the service being away is the restart, not an error")
        XCTAssertFalse(vm.canStartUpdate)
        await vm.poll()
        XCTAssertEqual(vm.phase, .restarting)
        XCTAssertEqual(cameBack, 0)
        await vm.poll()
        XCTAssertEqual(vm.phase, .applied)
        XCTAssertEqual(vm.appliedText, "ProgramUpdate_Applied 0.1.11")
        XCTAssertTrue(vm.canStartUpdate)
        XCTAssertEqual(cameBack, 1)
        XCTAssertFalse(cli.calls.contains("status --json"))
    }

    func test_fr15_a_new_service_with_nothing_to_report_runs_the_version_installed() async {
        let cli = SequenceCli()
        cli.then(update, installing)
        cli.thenFail(status, unreachable)
        cli.then(status, statusWith(nil))
        let vm = model(cli)
        await vm.updateNow()
        await vm.poll()
        await vm.poll()
        XCTAssertEqual(vm.phase, .applied)
        XCTAssertEqual(vm.appliedText, "ProgramUpdate_Applied 0.1.11")
    }

    func test_fr15_an_answer_lost_to_the_restart_is_no_failure() async {
        let cli = SequenceCli()
        cli.thenFail(update, "dsa: the service closed the connection")
        cli.then(status, statusWith("installing"))
        let vm = model(cli)
        let asking = await startHeld(vm, cli)
        await vm.poll()
        XCTAssertEqual(vm.phase, .installing)
        cli.release()
        let answer = await asking.value
        XCTAssertNil(answer)
        XCTAssertEqual(vm.phase, .restarting)
        XCTAssertNil(vm.error)
    }

    func test_fr15_a_refused_or_needless_update_shows_no_progress() async {
        let cli = SequenceCli()
        cli.thenFail(update, "dsa: DSA was not updated: these users' Android did not stop in time: alice (Stopping)")
        let vm = model(cli)
        let refused = await vm.updateNow()
        XCTAssertNil(refused)
        XCTAssertEqual(vm.phase, ProgramUpdatePhase.none)
        XCTAssertTrue(vm.error?.contains("did not stop in time") ?? false)
        XCTAssertTrue(vm.canStartUpdate)

        let upToDate = SequenceCli()
        upToDate.then(update, #"{"installing":null,"detail":"DSA 0.1.11 is up to date","status":null}"#)
        let fresh = model(upToDate)
        let answer = await fresh.updateNow()
        XCTAssertNil(answer?.installing)
        XCTAssertEqual(answer?.detail, "DSA 0.1.11 is up to date")
        XCTAssertEqual(fresh.phase, ProgramUpdatePhase.none)
        XCTAssertNil(fresh.error)
    }

    func test_fr15_a_failed_update_and_a_service_away_too_long_say_so() async {
        let cli = SequenceCli()
        cli.then(update, installing)
        cli.then(status, statusWith("failed"))
        let vm = model(cli)
        await vm.updateNow()
        await vm.poll()
        XCTAssertEqual(vm.phase, .failed)
        XCTAssertEqual(vm.failedText, "ProgramUpdate_Failed 0.1.11")
        XCTAssertTrue(vm.canStartUpdate)

        var now = Date(timeIntervalSince1970: 1_800_000_000)
        let away = SequenceCli()
        away.then(update, installing)
        away.thenFail(status, unreachable)
        away.thenFail(status, unreachable)
        away.then(status, statusWith("applied"))
        let late = model(away, now: { now })
        await late.updateNow()
        await late.poll()
        XCTAssertEqual(late.phase, .restarting)
        now += ProgramUpdateViewModel.restartLimit + 1
        await late.poll()
        XCTAssertEqual(late.phase, .failed)
        XCTAssertEqual(late.failedText, "ProgramUpdate_NotBack 0.1.11")
        await late.poll()
        XCTAssertEqual(late.phase, .applied, "a service that comes back late still says how it went")
    }

    func test_fr15_the_menu_bar_never_starts_the_service_while_an_update_runs() async {
        let cli = SequenceCli()
        cli.then(update, installing)
        cli.thenFail(status, unreachable)
        cli.then("status --json", statusWith(nil))
        let vm = model(cli)
        let general = GeneralViewModel(service: DsaService(cli: cli), text: KeyText())
        general.updating = { vm.inProgress }
        await general.poll()
        XCTAssertTrue(cli.calls.contains("status --json"))
        await vm.updateNow()
        let before = cli.calls.count
        await general.poll()
        XCTAssertEqual(Array(cli.calls[before...]), [status])
    }

    func test_fr15_each_phase_name_is_read() {
        XCTAssertEqual(ProgramUpdatePhase.parse(nil), ProgramUpdatePhase.none)
        XCTAssertEqual(ProgramUpdatePhase.parse("downloading"), .downloading)
        XCTAssertEqual(ProgramUpdatePhase.parse("stopping_android"), .stoppingAndroid)
        XCTAssertEqual(ProgramUpdatePhase.parse("installing"), .installing)
        XCTAssertEqual(ProgramUpdatePhase.parse("restarting"), .restarting)
        XCTAssertEqual(ProgramUpdatePhase.parse("applied"), .applied)
        XCTAssertEqual(ProgramUpdatePhase.parse("failed"), .failed)
        XCTAssertEqual(ProgramUpdatePhase.parse("starting"), ProgramUpdatePhase.none, "the app's own phase")
        XCTAssertEqual(ProgramUpdatePhase.parse("something newer"), ProgramUpdatePhase.none)
    }
}
