import XCTest

final class Scenarios: XCTestCase {
    private let app = XCUIApplication()

    override func setUpWithError() throws {
        continueAfterFailure = false
        // a fresh token for each run: the control channel exists in Debug builds only
        let token = (0..<32).map { _ in String(format: "%02x", UInt8.random(in: 0...255)) }.joined()
        app.launchEnvironment["FILMCRAFT_CONTROL_TOKEN"] = token
        app.launchEnvironment["FILMCRAFT_CONTROL_PORT"] = "9876"
        app.launch()
    }

    private func point(_ x: CGFloat, _ y: CGFloat) -> XCUICoordinate {
        app.coordinate(withNormalizedOffset: .zero).withOffset(CGVector(dx: x, dy: y))
    }

    @discardableResult
    private func tap(_ x: CGFloat, _ y: CGFloat) -> XCUICoordinate {
        let p = point(x, y)
        p.tap()
        return p
    }

    private func doubleTap(_ x: CGFloat, _ y: CGFloat) {
        point(x, y).doubleTap()
    }

    private func longPress(_ x: CGFloat, _ y: CGFloat, duration: TimeInterval = 0.6) {
        point(x, y).press(forDuration: duration)
    }

    private func drag(from: (CGFloat, CGFloat), to: (CGFloat, CGFloat)) {
        point(from.0, from.1).press(forDuration: 0.15, thenDragTo: point(to.0, to.1), withVelocity: .slow, thenHoldForDuration: 0.3)
    }

    private func shot(_ name: String) {
        let attachment = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        attachment.name = name
        attachment.lifetime = .keepAlways
        add(attachment)
    }

    func testSmoke() throws {
        XCTAssertTrue(app.wait(for: .runningForeground, timeout: 30))
        sleep(2)
        shot("smoke-launch")
        XCTAssertFalse(app.alerts.element.exists, "FilmCraft must not show a startup error alert")
    }

    func testTouchSmoke() throws {
        XCTAssertTrue(app.wait(for: .runningForeground, timeout: 30))
        sleep(2)
        tap(80, 80)
        doubleTap(160, 160)
        longPress(240, 240)
        drag(from: (300, 300), to: (500, 320))
        shot("touch-smoke")
        XCTAssertTrue(app.wait(for: .runningForeground, timeout: 2))
    }
}
