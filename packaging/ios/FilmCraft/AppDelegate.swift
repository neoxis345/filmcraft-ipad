// FilmCraft for iPadOS: the app and its scene (UIKit's scene life cycle, required by the iPadOS 27 SDK).

import AVFoundation
import UIKit

@main
final class AppDelegate: UIResponder, UIApplicationDelegate {
    func application(_ application: UIApplication, didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]?) -> Bool {
        configureAudioSession()
        return true
    }

    func application(_ application: UIApplication, configurationForConnecting connectingSceneSession: UISceneSession, options: UIScene.ConnectionOptions) -> UISceneConfiguration {
        let config = UISceneConfiguration(name: "Default", sessionRole: connectingSceneSession.role)
        config.delegateClass = SceneDelegate.self
        return config
    }

    /// Playback goes to the speakers, AirPlay or Bluetooth headphones and keeps playing with the
    /// silent switch; the microphone (voice-over) is only opened when recording.
    private func configureAudioSession() {
        let session = AVAudioSession.sharedInstance()
        do {
            try session.setCategory(.playAndRecord, mode: .default, options: [.defaultToSpeaker, .allowBluetoothA2DP, .allowAirPlay])
            try session.setActive(true)
        } catch {
            NSLog("FilmCraft: audio session: \(error)")
            try? session.setCategory(.playback)
            try? session.setActive(true)
        }
    }
}

final class SceneDelegate: UIResponder, UIWindowSceneDelegate {
    var window: UIWindow?
    private var controller: FilmCraftViewController?

    func scene(_ scene: UIScene, willConnectTo session: UISceneSession, options connectionOptions: UIScene.ConnectionOptions) {
        guard let windowScene = scene as? UIWindowScene else { return }
        // An editing interface needs room: with Stage Manager or windows side by side, iPadOS keeps
        // FilmCraft's window at least this big (the iPad mini in portrait is 744 points wide).
        windowScene.sizeRestrictions?.minimumSize = CGSize(width: 700, height: 500)
        let window = UIWindow(windowScene: windowScene)
        let controller = FilmCraftViewController()
        window.rootViewController = controller
        window.makeKeyAndVisible()
        self.window = window
        self.controller = controller
        // files FilmCraft was launched to open (Files ▸ Share ▸ FilmCraft)
        if !connectionOptions.urlContexts.isEmpty {
            controller.open(urls: connectionOptions.urlContexts.map(\.url))
        }
    }

    func scene(_ scene: UIScene, openURLContexts URLContexts: Set<UIOpenURLContext>) {
        controller?.open(urls: URLContexts.map(\.url))
    }

    func sceneDidBecomeActive(_ scene: UIScene) {
        controller?.setActive(true)
    }

    func sceneWillResignActive(_ scene: UIScene) {
        // Control Center, a notification, the app switcher: keep drawing until really in the background
    }

    func sceneDidEnterBackground(_ scene: UIScene) {
        controller?.setActive(false)
    }

    func sceneWillEnterForeground(_ scene: UIScene) {
        controller?.setActive(true)
    }

    func sceneDidDisconnect(_ scene: UIScene) {
        controller?.shutdown()
    }
}
