import XCTest

/// The sidebar's order, read from the app's source: the page list lives in the app target,
/// which the tests cannot import.
final class SidebarOrderTests: XCTestCase {
    private var package: URL {
        URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
    }

    private func window() throws -> String {
        try String(contentsOf: package.appendingPathComponent("Sources/DsaSettings/SettingsWindow.swift"), encoding: .utf8)
    }

    func test_fr17_logs_sits_directly_above_about_in_the_sidebars_last_group() throws {
        let source = try window()
        XCTAssertTrue(
            source.contains("case general, memory, apps, background, widgets, display, identity, network, updates, advanced, logs, about"),
            "the pages are not in the Windows app's order, with Logs directly above About at the end")
        XCTAssertTrue(
            source.contains("static let footer: [SettingsPage] = [.logs, .about]"),
            "the sidebar's last group is not Logs and then About")
        XCTAssertTrue(source.contains("ForEach(SettingsPage.main)"), "the first group does not list the pages above Logs")
        XCTAssertTrue(source.contains("ForEach(SettingsPage.footer)"), "the last group does not list Logs and About")
        let main = try XCTUnwrap(source.range(of: "ForEach(SettingsPage.main)"))
        let footer = try XCTUnwrap(source.range(of: "ForEach(SettingsPage.footer)"))
        XCTAssertLessThan(main.lowerBound, footer.lowerBound, "Logs and About are not the sidebar's last group")
    }
}
