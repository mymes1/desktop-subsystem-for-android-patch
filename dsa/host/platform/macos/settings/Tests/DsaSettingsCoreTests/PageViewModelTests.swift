import XCTest
@testable import DsaSettingsCore

/// The Apps, Widgets, Background, Device identity, Android, Android log pages and the APK
/// drop, through the dsa command.
@MainActor
final class PageViewModelTests: XCTestCase {
    private let app = #"{"package":"org.example.notes","label":"Notes","version_name":"2.1","system_app":false,"app_bytes":2097152}"#
    private let system = #"{"package":"com.android.settings","label":"Settings","system_app":true}"#

    func test_fr17_apps_page_finds_apps_by_name_or_package() async {
        let cli = FakeCli()
        cli.answer("apps list --json", #"{"apps":[\#(app),\#(system)]}"#)
        let apps = AppsViewModel(service: AppsService(cli: cli), text: KeyText())
        await apps.refresh()
        XCTAssertEqual(apps.rows.map(\.label), ["Notes", "Settings"])
        XCTAssertEqual(apps.count, "Apps_Count")
        apps.search = "EXAMPLE"
        XCTAssertEqual(apps.rows.map(\.package), ["org.example.notes"])
        XCTAssertEqual(apps.count, "Apps_CountMatching")
        apps.search = "nothing"
        XCTAssertTrue(apps.hasNoMatches)
        apps.search = ""
        let settings = apps.rows.first { $0.package == "com.android.settings" }
        XCTAssertEqual(settings?.canUninstall, false)
        XCTAssertEqual(settings?.uninstallTip, "Apps_SystemTip")
    }

    func test_fr17_an_uninstall_android_refuses_says_why() async {
        let cli = FakeCli()
        cli.answer("apps list --json", #"{"apps":[\#(app)]}"#)
        cli.fail("apps uninstall org.example.notes --json", "dsa: not uninstalled (blocked): DELETE_FAILED_DEVICE_POLICY_MANAGER")
        let apps = AppsViewModel(service: AppsService(cli: cli), text: KeyText())
        await apps.refresh()
        await apps.uninstall("org.example.notes")
        XCTAssertEqual(apps.error, "Apps_NotUninstalled")
        XCTAssertNil(apps.message)
    }

    func test_fr17_app_window_settings_go_through_app_window() async {
        let cli = FakeCli()
        cli.answer("apps list --json", #"{"apps":[\#(app)]}"#)
        cli.answer("app-window --json", #"{"apps":[]}"#)
        cli.answer("app-window org.example.notes --fixed-size off --start-size 412x915 --json",
                   #"{"apps":[{"package":"org.example.notes","fixed_size":false,"start_size":[412,915]}]}"#)
        let apps = AppsViewModel(service: AppsService(cli: cli), text: KeyText())
        await apps.refresh()
        await apps.loadWindowSettings()
        XCTAssertEqual(apps.rows.first?.startSizeIndex, 0)
        await apps.setStartSize("org.example.notes", 1)
        XCTAssertEqual(apps.rows.first?.startSizeIndex, 1)
        XCTAssertEqual(apps.message, "Apps_WindowChanged")
    }

    func test_fr17_widgets_placed_elsewhere_show_on_the_page() async {
        let cli = FakeCli()
        cli.answer("widgets list --json", #"{"providers":[{"provider":"org.example/.Clock","package":"org.example","label":"Clock","default_width_dp":110,"default_height_dp":40,"needs_configuration":true}],"widgets":[]}"#)
        cli.streams["widgets watch --json"] = [
            #"{"event":"placed","widget":{"id":3,"provider":"org.example/.Clock","label":"Clock","placement":{"x":10,"y":20,"width_dp":110,"height_dp":40}}}"#,
            "not json",
            #"{"event":"placed","widget":{"id":1,"provider":"org.example/.Clock","label":"Clock"}}"#,
            #"{"event":"removed","id":3}"#,
        ]
        let widgets = WidgetsViewModel(service: DsaService(cli: cli), text: KeyText(), stream: cli)
        await widgets.refresh()
        XCTAssertTrue(widgets.hasNoPlaced)
        XCTAssertEqual(widgets.offers.first?.app, "org.example")
        XCTAssertEqual(widgets.offers.first?.detail, "Widgets_Size · Widgets_Setup")
        try? await widgets.watchOnce()
        XCTAssertEqual(widgets.placed.map(\.id), [1])
        XCTAssertEqual(widgets.offers.count, 1)
    }

    func test_fr17_background_apps_show_what_they_play() async {
        let cli = FakeCli()
        cli.answer("settings --json", #"{"stop_without_asking":true}"#)
        cli.answer("background list --json", #"{"apps":[{"package":"org.example.music","label":"Music","title":"Playing","text":""}],"media":[{"package":"org.example.music","title":"Song","artist":"Band","playback":"playing"}]}"#)
        let background = BackgroundViewModel(service: DsaService(cli: cli), text: KeyText())
        await background.refresh()
        XCTAssertFalse(background.askBeforeStop)
        XCTAssertEqual(background.apps.first?.detail, "Playing\nBackground_Playing")
        cli.answer("background stop org.example.music --json", "{}")
        await background.stop("org.example.music")
        XCTAssertEqual(background.message, "Background_Stopped")
    }

    func test_fr17_device_identity_checks_values_before_sending_them() {
        XCTAssertNil(DeviceIdentityRules.check("model", ""))
        XCTAssertNil(DeviceIdentityRules.check("model", "Pixel 9"))
        XCTAssertEqual(DeviceIdentityRules.check("model", " Pixel"), "Identity_model_Invalid")
        XCTAssertEqual(DeviceIdentityRules.check("brand", "-x"), "Identity_brand_Invalid")
        XCTAssertNil(DeviceIdentityRules.check("characteristics", "tablet,nosdcard"))
        XCTAssertEqual(DeviceIdentityRules.check("fingerprint.type", "debug"), "Identity_fingerprint_type_Invalid")
        XCTAssertTrue(DeviceIdentityRules.isFingerprint("google/panther/panther:14/UP1A/123:user/release-keys"))
        XCTAssertFalse(DeviceIdentityRules.isFingerprint("google/panther:14/UP1A/123:user/release-keys"))
    }

    func test_fr17_device_identity_sends_only_what_changed() async {
        let cli = FakeCli()
        let report = #"{"chosen":{},"effective":{"brand":"dsa","name":"dsa_x86_64","device":"dsa","fingerprint_release":"15","fingerprint_id":"AP3A","fingerprint_incremental":"1","fingerprint_type":"user","fingerprint_tags":"release-keys","fingerprint":"dsa/dsa_x86_64/dsa:15/AP3A/1:user/release-keys"}}"#
        cli.answer("settings device --json", report)
        cli.answer("status --json", #"{"state":"Active"}"#)
        cli.answer("settings set device.brand acme --json", #"{"chosen":{"brand":"acme"},"effective":{"brand":"acme"}}"#)
        let identity = DeviceIdentityViewModel(service: DsaService(cli: cli), text: KeyText())
        await identity.load()
        XCTAssertFalse(identity.canSave)
        identity.edit("brand", "acme")
        XCTAssertEqual(identity.fingerprintPreview, "acme/dsa_x86_64/dsa:15/AP3A/1:user/release-keys")
        identity.edit("model", " bad")
        XCTAssertFalse(identity.canSave)
        XCTAssertEqual(identity.problem(identity.field("model")), "Identity_model_Invalid")
        identity.edit("model", "")
        XCTAssertTrue(identity.canSave)
        await identity.save()
        let changes = cli.calls.filter { $0.starts(with: ["settings", "set"]) || $0.starts(with: ["settings", "reset"]) }
        XCTAssertEqual(changes.map { $0.joined(separator: " ") },
                       ["settings set device.brand acme --json"])
        XCTAssertEqual(identity.message, "Identity_SavedRestart")
        XCTAssertTrue(identity.needsRestart)
    }

    func test_fr17_moving_to_an_older_android_needs_the_data_erased() async {
        let cli = FakeCli()
        cli.answer("android version --json", #"{"version":"16.0.1","android_major":16,"channel":"latest","supported_majors":[15,16],"target_major":16}"#)
        let android = AndroidVersionViewModel(cli: cli, text: KeyText())
        await android.load()
        XCTAssertEqual(android.choices.map(\.channel), ["latest", "16", "15"])
        XCTAssertEqual(android.selectedChannel, "latest")
        XCTAssertFalse(android.canApply)
        android.selectedChannel = "15"
        XCTAssertTrue(android.canApply)
        XCTAssertEqual(android.selectedMove, .down)
        XCTAssertEqual(AndroidVersionViewModel.useArguments(android.selected!, eraseData: true),
                       ["android", "use", "15", "--erase-data", "--json"])
        XCTAssertFalse(android.isSwitching)
    }

    func test_fr17_log_page_filters_and_pauses() {
        let cli = FakeCli()
        let logs = LogViewModel(stream: cli, service: DsaService(cli: cli), text: KeyText())
        logs.addMany([
            #"{"time":"01-01 00:00:00.000","priority":"I","tag":"ActivityManager","pid":12,"tid":34,"message":"Start proc"}"#,
            #"{"time":"01-01 00:00:00.001","priority":"W","tag":"Zygote","pid":1,"tid":1,"message":"slow"}"#,
            "garbage",
        ])
        XCTAssertEqual(logs.lines.count, 2)
        XCTAssertEqual(logs.lines.first?.text, "01-01 00:00:00.000    12    34 I ActivityManager: Start proc")
        logs.filter = "zygote"
        XCTAssertEqual(logs.lines.map(\.tag), ["Zygote"])
        logs.paused = true
        logs.add(#"{"tag":"Zygote","message":"again"}"#)
        XCTAssertEqual(logs.waiting, 1)
        XCTAssertEqual(logs.lines.count, 1)
        logs.paused = false
        XCTAssertEqual(logs.lines.count, 2)
        XCTAssertEqual(LogViewModel.arguments("W"), ["logcat", "--json", "*:W"])
    }

    func test_fr17_log_page_reads_the_log_through_dsa_logcat() async throws {
        let cli = FakeCli()
        cli.streams["logcat --json *:I"] = [#"{"tag":"A","message":"one"}"#, #"{"tag":"B","message":"two"}"#]
        let logs = LogViewModel(stream: cli, service: DsaService(cli: cli), text: KeyText())
        logs.start()
        for _ in 0..<50 where logs.isReading {
            try await Task.sleep(nanoseconds: 20_000_000)
        }
        XCTAssertEqual(logs.lines.map(\.tag), ["A", "B"])
        XCTAssertFalse(logs.isReading)
    }

    func test_fr17_a_dropped_apk_is_installed_and_others_are_named() async {
        let cli = FakeCli()
        cli.answer("install /tmp/notes.apk --json", #"{"installed":true,"status":"installed","package":"org.example.notes"}"#)
        cli.fail("install /tmp/old.apk --json", "dsa: not installed (downgrade): INSTALL_FAILED_VERSION_DOWNGRADE")
        let install = InstallViewModel(service: DsaService(cli: cli), text: KeyText())
        XCTAssertTrue(install.accepts(["/tmp/notes.APK"]))
        XCTAssertFalse(install.accepts(["/tmp/notes.txt"]))
        await install.drop(["/tmp/notes.apk", "/tmp/old.apk", "/tmp/readme.txt"])
        XCTAssertEqual(install.severity, .error)
        XCTAssertEqual(install.title, "Install_FailedTitle")
        XCTAssertEqual(install.message, "Install_Installed\nInstall_Refused\nInstall_NotApk")
        XCTAssertTrue(install.isOpen)
    }

    func test_fr17_dsa_is_found_beside_the_app_first() {
        let found = ProcessDsaCli.locate(environment: [:], programFolder: URL(fileURLWithPath: "/Applications/DSA.app/Contents/MacOS")) {
            $0 == "/Applications/DSA.app/Contents/MacOS/dsa"
        }
        XCTAssertEqual(found, "/Applications/DSA.app/Contents/MacOS/dsa")
        XCTAssertEqual(ProcessDsaCli.locate(environment: ["DSA_CLI": "/x/dsa"], programFolder: nil) { _ in false }, "/x/dsa")
    }
}
