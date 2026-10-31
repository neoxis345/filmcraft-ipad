// The FilmCraft screen: a Metal view the Rust host draws into, a native toolbar for touch, and every
// input (fingers, Apple Pencil, trackpad, mouse, hardware and on-screen keyboards) passed to Rust.

import QuartzCore
import UIKit

/// A view backed by a CAMetalLayer (wgpu draws on it).
final class MetalView: UIView {
    override class var layerClass: AnyClass { CAMetalLayer.self }
    var metalLayer: CAMetalLayer { layer as! CAMetalLayer }
}

/// Takes the on-screen keyboard's typing while a FilmCraft text field is focused.
final class KeyboardProxy: UIView, UIKeyInput {
    var onText: ((String) -> Void)?
    var onBackspace: (() -> Void)?
    override var canBecomeFirstResponder: Bool { true }
    var hasText: Bool { true }
    func insertText(_ text: String) { onText?(text) }
    func deleteBackward() { onBackspace?() }
    // names, titles and captions: no autocorrection rewriting what the user types
    var autocorrectionType: UITextAutocorrectionType = .no
    var autocapitalizationType: UITextAutocapitalizationType = .sentences
    var spellCheckingType: UITextSpellCheckingType = .no
    var smartQuotesType: UITextSmartQuotesType = .no
    var smartDashesType: UITextSmartDashesType = .no
    var smartInsertDeleteType: UITextSmartInsertDeleteType = .no
    var keyboardType: UIKeyboardType = .default
}

final class FilmCraftViewController: UIViewController, UIGestureRecognizerDelegate, UIPointerInteractionDelegate {
    /// The controller on screen (the bridge's calls from Rust go to it).
    static weak var current: FilmCraftViewController?

    private let metalView = MetalView()
    private let keyboardProxy = KeyboardProxy()
    private let toolbar = UIScrollView()
    private let toolbarStack = UIStackView()
    private var playButton: UIButton?
    private var host: OpaquePointer?
    private var link: CADisplayLink?
    private var wakeTimer: Timer?
    private var nextFrameAt: CFTimeInterval = 0
    private var active = true
    private var maxFps = 60
    private var lastSize: (UInt32, UInt32, Float) = (0, 0, 0)
    private var pendingURLs: [URL] = []
    private var repeatTimers: [UInt32: Timer] = [:]
    /// UIKit can clear `UIEvent.buttonMask` before the final indirect-pointer sample. Keep the
    /// button chosen at began so a secondary-button drag always releases as a secondary button.
    private var pointerButtons: [UInt64: Bool] = [:]
    private var lastScroll = CGPoint.zero
    private var lastPinch: CGFloat = 1
    /// The pointer shape FilmCraft asks for (host.rs `pointer_shape`), and the interaction drawing it.
    private var cursor = "default"
    private var pointerInteraction: UIPointerInteraction?
    private var startFailed = false
    let dialogs = Dialogs()

    static let toolbarHeight: CGFloat = 44

    override func viewDidLoad() {
        super.viewDidLoad()
        FilmCraftViewController.current = self
        view.backgroundColor = UIColor(white: 0.11, alpha: 1)
        dialogs.controller = self

        metalView.isMultipleTouchEnabled = true
        metalView.isExclusiveTouch = false
        metalView.metalLayer.isOpaque = true
        metalView.metalLayer.framebufferOnly = true
        metalView.metalLayer.maximumDrawableCount = 3
        metalView.metalLayer.presentsWithTransaction = false
        view.addSubview(metalView)

        keyboardProxy.frame = CGRect(x: 0, y: 0, width: 1, height: 1)
        keyboardProxy.alpha = 0.01
        keyboardProxy.isUserInteractionEnabled = false
        keyboardProxy.onText = { [weak self] text in self?.send { fc_host_text($0, text) } }
        keyboardProxy.onBackspace = { [weak self] in self?.send { fc_host_backspace($0) } }
        view.addSubview(keyboardProxy)

        buildToolbar()
        installGestures()

        let nc = NotificationCenter.default
        nc.addObserver(self, selector: #selector(memoryWarning), name: UIApplication.didReceiveMemoryWarningNotification, object: nil)
        nc.addObserver(self, selector: #selector(thermalChanged), name: ProcessInfo.thermalStateDidChangeNotification, object: nil)
        nc.addObserver(self, selector: #selector(powerChanged), name: .NSProcessInfoPowerStateDidChange, object: nil)
    }

    // MARK: - Layout and start

    override var prefersHomeIndicatorAutoHidden: Bool { true }
    /// Swipes at the screen edges reach the timeline first; a second swipe goes to iPadOS.
    override var preferredScreenEdgesDeferringSystemGestures: UIRectEdge { .all }
    override var prefersStatusBarHidden: Bool { false }
    override var preferredStatusBarStyle: UIStatusBarStyle { .lightContent }
    override var canBecomeFirstResponder: Bool { true }

    override func viewDidLayoutSubviews() {
        super.viewDidLayoutSubviews()
        let safe = view.bounds.inset(by: view.safeAreaInsets)
        let h = FilmCraftViewController.toolbarHeight
        toolbar.frame = CGRect(x: safe.minX, y: safe.maxY - h, width: safe.width, height: h)
        metalView.frame = CGRect(x: safe.minX, y: safe.minY, width: safe.width, height: max(1, safe.height - h))
        let scale = view.window?.screen.scale ?? traitCollection.displayScale
        metalView.contentScaleFactor = scale
        let w = UInt32(max(1, (metalView.bounds.width * scale).rounded()))
        let hpx = UInt32(max(1, (metalView.bounds.height * scale).rounded()))
        metalView.metalLayer.drawableSize = CGSize(width: CGFloat(w), height: CGFloat(hpx))
        let size = (w, hpx, Float(scale))
        if host == nil {
            startIfReady(size: size)
        } else if size != lastSize {
            fc_host_resize(host, w, hpx, Float(scale))
            requestFrame(after: 0)
        }
        lastSize = size
        let content = toolbarStack.systemLayoutSizeFitting(CGSize(width: CGFloat.greatestFiniteMagnitude, height: h))
        toolbarStack.frame = CGRect(x: 0, y: 0, width: max(content.width, toolbar.bounds.width), height: h)
        toolbar.contentSize = toolbarStack.frame.size
    }

    override func viewDidAppear(_ animated: Bool) {
        super.viewDidAppear(animated)
        becomeFirstResponder()
    }

    private func startIfReady(size: (UInt32, UInt32, Float)) {
        guard host == nil, !startFailed, metalView.bounds.width > 1, metalView.bounds.height > 1 else { return }
        let documents = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
        let languages = (try? JSONSerialization.data(withJSONObject: Locale.preferredLanguages)).flatMap { String(data: $0, encoding: .utf8) } ?? "[]"
        let layer = Unmanaged.passUnretained(metalView.metalLayer).toOpaque()
        host = documents.path.withCString { docs in
            languages.withCString { langs in
                fc_host_create(layer, size.0, size.1, size.2, docs, langs, ProcessInfo.processInfo.physicalMemory)
            }
        }
        guard host != nil else {
            startFailed = true
            let alert = UIAlertController(title: "FilmCraft n'a pas pu démarrer", message: "Le processeur graphique n'a pas pu être initialisé. Redémarrez l'iPad puis rouvrez FilmCraft.", preferredStyle: .alert)
            alert.addAction(UIAlertAction(title: "OK", style: .default))
            present(alert, animated: true)
            return
        }
        thermalChanged()
        powerChanged()
        let link = CADisplayLink(target: self, selector: #selector(tick))
        link.preferredFrameRateRange = CAFrameRateRange(minimum: 30, maximum: Float(maxFps), preferred: Float(maxFps))
        link.add(to: .main, forMode: .common)
        self.link = link
        if !pendingURLs.isEmpty {
            open(urls: pendingURLs)
            pendingURLs = []
        }
        requestFrame(after: 0)
    }

    // MARK: - Frames

    /// Ask for a frame `delay` seconds from now (infinite: none).
    func requestFrame(after delay: Double) {
        guard delay.isFinite else { return }
        let at = CACurrentMediaTime() + max(0, delay)
        if at < nextFrameAt || nextFrameAt <= 0 { nextFrameAt = at }
        updateLink()
    }

    /// Run the display link only while frames are due soon; sleep otherwise (battery, heat).
    private func updateLink() {
        guard let link else { return }
        wakeTimer?.invalidate()
        wakeTimer = nil
        guard active, host != nil else {
            link.isPaused = true
            return
        }
        let wait = nextFrameAt - CACurrentMediaTime()
        if nextFrameAt > 0 && wait <= 0.2 {
            link.isPaused = false
        } else {
            link.isPaused = true
            if nextFrameAt > 0, wait.isFinite {
                let t = Timer(timeInterval: max(0.01, wait - 0.05), repeats: false) { [weak self] _ in self?.updateLink() }
                RunLoop.main.add(t, forMode: .common)
                wakeTimer = t
            }
        }
    }

    @objc private func tick() {
        guard let host, active else { return }
        let now = CACurrentMediaTime()
        if nextFrameAt <= 0 || now + 0.002 < nextFrameAt {
            updateLink()
            return
        }
        nextFrameAt = 0
        let next = fc_host_frame(host)
        if next.isFinite {
            requestFrame(after: next)
        } else {
            updateLink()
        }
    }

    /// Run `body` with the host and draw soon after.
    func send(_ body: (OpaquePointer) -> Void) {
        guard let host else { return }
        body(host)
        requestFrame(after: 0)
    }

    func setActive(_ on: Bool) {
        if !on { stopAllRepeatTimers() }
        guard on != active else { return }
        active = on
        if on {
            send { fc_host_set_active($0, true) }
        } else if let host {
            // a few seconds to write the recovery journal before iPadOS may suspend the app
            let task = UIApplication.shared.beginBackgroundTask(withName: "FilmCraft journal")
            fc_host_set_active(host, false)
            UIApplication.shared.endBackgroundTask(task)
        }
        updateLink()
    }

    func shutdown() {
        link?.invalidate()
        link = nil
        if let host {
            fc_host_destroy(host)
        }
        host = nil
    }

    // MARK: - Status from Rust

    func apply(status: [String: Any]) {
        if let asked = status["maxFps"] as? Int {
            // FilmCraft asks for up to 120 (ProMotion): every other iPad stays at its screen's rate
            let screenMax = view.window?.windowScene?.screen.maximumFramesPerSecond ?? 60
            let fps = max(30, min(asked, screenMax))
            if fps != maxFps {
                maxFps = fps
                link?.preferredFrameRateRange = CAFrameRateRange(minimum: Float(min(30, fps)), maximum: Float(fps), preferred: Float(fps))
            }
        }
        if let playing = status["playing"] as? Bool {
            let busy = status["busy"] as? Bool ?? false
            playButton?.configuration?.image = UIImage(systemName: playing ? "pause.fill" : "play.fill")
            playButton?.configuration?.title = playing ? "Pause" : "Lecture"
            // the screen stays on while a video plays or an export runs
            UIApplication.shared.isIdleTimerDisabled = playing || busy
        }
        if let c = status["cursor"] as? String, c != cursor {
            cursor = c
            pointerInteraction?.invalidate()
        }
    }

    func setKeyboardVisible(_ visible: Bool) {
        if visible {
            keyboardProxy.becomeFirstResponder()
        } else if keyboardProxy.isFirstResponder {
            keyboardProxy.resignFirstResponder()
            becomeFirstResponder()
        }
    }

    // MARK: - Files from outside

    func open(urls: [URL]) {
        guard host != nil else {
            pendingURLs += urls
            return
        }
        Dialogs.importCopies(of: urls, from: self) { [weak self] paths in
            guard let self, !paths.isEmpty else { return }
            let json = Dialogs.json(paths)
            self.send { host in json.withCString { fc_host_open_paths(host, $0) } }
        }
    }

    // MARK: - System state

    @objc private func memoryWarning() {
        send { fc_host_memory_warning($0) }
    }

    @objc private func thermalChanged() {
        let state: Int32
        switch ProcessInfo.processInfo.thermalState {
        case .nominal: state = 0
        case .fair: state = 1
        case .serious: state = 2
        case .critical: state = 3
        @unknown default: state = 0
        }
        DispatchQueue.main.async { [weak self] in self?.send { fc_host_thermal($0, state) } }
    }

    @objc private func powerChanged() {
        let on = ProcessInfo.processInfo.isLowPowerModeEnabled
        DispatchQueue.main.async { [weak self] in self?.send { fc_host_low_power($0, on) } }
    }

    // MARK: - Toolbar

    private struct Tool {
        let symbol: String
        let title: String
        let command: String
    }

    private func buildToolbar() {
        toolbar.showsHorizontalScrollIndicator = false
        toolbar.backgroundColor = UIColor(white: 0.08, alpha: 1)
        toolbar.alwaysBounceHorizontal = false
        toolbarStack.axis = .horizontal
        toolbarStack.alignment = .center
        toolbarStack.spacing = 2
        toolbarStack.isLayoutMarginsRelativeArrangement = true
        toolbarStack.directionalLayoutMargins = NSDirectionalEdgeInsets(top: 0, leading: 8, bottom: 0, trailing: 8)
        toolbar.addSubview(toolbarStack)
        view.addSubview(toolbar)

        let groups: [[Tool]] = [
            [
                Tool(symbol: "square.and.arrow.down", title: "Importer", command: "file.import"),
                Tool(symbol: "externaldrive", title: "Enregistrer", command: "file.save"),
                Tool(symbol: "square.and.arrow.up", title: "Exporter", command: "file.exportMedia"),
                Tool(symbol: "square.and.arrow.up.on.square", title: "Partager", command: "ios.shareLastExport"),
            ],
            [
                Tool(symbol: "arrow.uturn.backward", title: "Annuler", command: "edit.undo"),
                Tool(symbol: "arrow.uturn.forward", title: "Rétablir", command: "edit.redo"),
            ],
            [
                Tool(symbol: "cursorarrow", title: "Sélection", command: "tool.selection"),
                Tool(symbol: "scissors", title: "Lame", command: "tool.razor"),
                Tool(symbol: "arrow.left.and.right", title: "Propagation", command: "tool.ripple"),
                Tool(symbol: "hand.raised", title: "Main", command: "tool.hand"),
            ],
            [
                Tool(symbol: "chevron.left.to.line", title: "Entrée", command: "markers.markIn"),
                Tool(symbol: "chevron.right.to.line", title: "Sortie", command: "markers.markOut"),
                Tool(symbol: "square.split.2x1", title: "Couper ici", command: "sequence.addEdit"),
                Tool(symbol: "trash", title: "Suppr. raccord", command: "edit.rippleDelete"),
                Tool(symbol: "bookmark", title: "Marqueur", command: "markers.add"),
            ],
            [
                Tool(symbol: "backward.frame", title: "Image −1", command: "playhead.stepBack"),
                Tool(symbol: "play.fill", title: "Lecture", command: "playback.toggle"),
                Tool(symbol: "forward.frame", title: "Image +1", command: "playhead.stepForward"),
            ],
            [
                Tool(symbol: "minus.magnifyingglass", title: "Zoom −", command: "view.zoomOut"),
                Tool(symbol: "plus.magnifyingglass", title: "Zoom +", command: "view.zoomIn"),
                Tool(symbol: "arrow.left.and.right.square", title: "Tout voir", command: "view.zoomToSequence"),
            ],
        ]
        for (i, group) in groups.enumerated() {
            if i > 0 {
                let sep = UIView()
                sep.backgroundColor = UIColor(white: 1, alpha: 0.12)
                sep.translatesAutoresizingMaskIntoConstraints = false
                sep.widthAnchor.constraint(equalToConstant: 1).isActive = true
                sep.heightAnchor.constraint(equalToConstant: 30).isActive = true
                toolbarStack.addArrangedSubview(sep)
                toolbarStack.setCustomSpacing(8, after: toolbarStack.arrangedSubviews[toolbarStack.arrangedSubviews.count - 2])
                toolbarStack.setCustomSpacing(8, after: sep)
            }
            for tool in group {
                let button = makeButton(tool)
                if tool.command == "playback.toggle" { playButton = button }
                toolbarStack.addArrangedSubview(button)
            }
        }
    }

    private func makeButton(_ tool: Tool) -> UIButton {
        var config = UIButton.Configuration.plain()
        config.image = UIImage(systemName: tool.symbol, withConfiguration: UIImage.SymbolConfiguration(pointSize: 17, weight: .regular))
        config.imagePlacement = .top
        config.imagePadding = 3
        config.title = tool.title
        config.titleTextAttributesTransformer = UIConfigurationTextAttributesTransformer { attrs in
            var a = attrs
            a.font = UIFont.systemFont(ofSize: 10, weight: .medium)
            return a
        }
        config.baseForegroundColor = UIColor(white: 0.9, alpha: 1)
        config.contentInsets = NSDirectionalEdgeInsets(top: 4, leading: 6, bottom: 4, trailing: 6)
        let command = tool.command
        let button = UIButton(configuration: config, primaryAction: UIAction { [weak self] _ in
            self?.send { host in command.withCString { fc_host_command(host, $0) } }
        })
        button.accessibilityLabel = tool.title
        button.toolTip = tool.title
        button.widthAnchor.constraint(greaterThanOrEqualToConstant: 54).isActive = true
        return button
    }

    // MARK: - Touches, Pencil, trackpad

    private func touchID(_ t: UITouch) -> UInt64 {
        UInt64(UInt(bitPattern: ObjectIdentifier(t).hashValue))
    }

    private func kind(_ t: UITouch) -> Int32 {
        switch t.type {
        case .pencil: return 1
        case .indirectPointer: return 2
        default: return 0
        }
    }

    private func forward(_ touches: Set<UITouch>, _ event: UIEvent?, phase: Int32) {
        guard let host else { return }
        for t in touches {
            let id = touchID(t)
            let pointer = kind(t) == 2
            let secondary: Bool
            if pointer {
                if phase == 0 {
                    let current = event?.buttonMask.contains(.secondary) ?? false
                    pointerButtons[id] = current
                    secondary = current
                } else {
                    secondary = pointerButtons[id] ?? (event?.buttonMask.contains(.secondary) ?? false)
                }
            } else {
                secondary = false
            }
            // the Pencil reports up to 240 positions a second: all of them, for smooth trims and drags
            let samples = phase == 1 ? (event?.coalescedTouches(for: t) ?? [t]) : [t]
            for s in samples {
                let p = s.type == .pencil ? s.preciseLocation(in: metalView) : s.location(in: metalView)
                fc_host_touch(host, id, phase, Float(p.x), Float(p.y), kind(t), secondary)
            }
            if pointer && (phase == 2 || phase == 3) {
                pointerButtons.removeValue(forKey: id)
            }
        }
        requestFrame(after: 0)
    }

    override func touchesBegan(_ touches: Set<UITouch>, with event: UIEvent?) { forward(touches, event, phase: 0) }
    override func touchesMoved(_ touches: Set<UITouch>, with event: UIEvent?) { forward(touches, event, phase: 1) }
    override func touchesEnded(_ touches: Set<UITouch>, with event: UIEvent?) { forward(touches, event, phase: 2) }
    override func touchesCancelled(_ touches: Set<UITouch>, with event: UIEvent?) { forward(touches, event, phase: 3) }

    private func installGestures() {
        // trackpad and mouse: hover, two-finger scroll / wheel, pinch (fingers on the glass are
        // handled from the raw touches above)
        let hover = UIHoverGestureRecognizer(target: self, action: #selector(hovered(_:)))
        metalView.addGestureRecognizer(hover)

        // the pointer's shape follows FilmCraft (resize arrows on clip edges, a beam over text…)
        let pointer = UIPointerInteraction(delegate: self)
        metalView.addInteraction(pointer)
        pointerInteraction = pointer

        // scroll events only (wheel, two fingers on a trackpad): no touch types, so a mouse
        // click-drag is a drag in FilmCraft and never also a scroll
        let scroll = UIPanGestureRecognizer(target: self, action: #selector(scrolled(_:)))
        scroll.allowedScrollTypesMask = .all
        scroll.allowedTouchTypes = []
        scroll.cancelsTouchesInView = false
        scroll.delegate = self
        metalView.addGestureRecognizer(scroll)

        let pinch = UIPinchGestureRecognizer(target: self, action: #selector(pinched(_:)))
        pinch.allowedTouchTypes = [NSNumber(value: UITouch.TouchType.indirectPointer.rawValue)]
        pinch.cancelsTouchesInView = false
        pinch.delegate = self
        metalView.addGestureRecognizer(pinch)
    }

    func pointerInteraction(_ interaction: UIPointerInteraction, regionFor request: UIPointerRegionRequest, defaultRegion: UIPointerRegion) -> UIPointerRegion? {
        // one region per shape: a new shape is a new region, so iPadOS redraws the pointer at once
        UIPointerRegion(rect: metalView.bounds, identifier: cursor as NSString)
    }

    func pointerInteraction(_ interaction: UIPointerInteraction, styleFor region: UIPointerRegion) -> UIPointerStyle? {
        let beam = UIPointerShape.roundedRect(CGRect(x: -2, y: -2, width: 4, height: 4), radius: 2)
        switch cursor {
        case "hidden":
            return .hidden()
        case "text":
            return UIPointerStyle(shape: .verticalBeam(length: 20), constrainedAxes: [])
        case "resizeH":
            let s = UIPointerStyle(shape: beam, constrainedAxes: [])
            s.accessories = [.arrow(.left), .arrow(.right)]
            return s
        case "resizeV":
            let s = UIPointerStyle(shape: beam, constrainedAxes: [])
            s.accessories = [.arrow(.top), .arrow(.bottom)]
            return s
        case "resizeD":
            let s = UIPointerStyle(shape: beam, constrainedAxes: [])
            s.accessories = [.arrow(.topLeft), .arrow(.bottomRight)]
            return s
        case "move":
            let s = UIPointerStyle(shape: beam, constrainedAxes: [])
            s.accessories = [.arrow(.right), .arrow(.bottom), .arrow(.left), .arrow(.top)]
            return s
        case "crosshair":
            return UIPointerStyle(shape: .roundedRect(CGRect(x: -3, y: -3, width: 6, height: 6), radius: 0), constrainedAxes: [])
        default:
            return nil
        }
    }

    func gestureRecognizer(_ g: UIGestureRecognizer, shouldRecognizeSimultaneouslyWith other: UIGestureRecognizer) -> Bool { true }

    @objc private func hovered(_ g: UIHoverGestureRecognizer) {
        let p = g.location(in: metalView)
        let inside = g.state == .began || g.state == .changed
        send { fc_host_hover($0, Float(p.x), Float(p.y), inside) }
    }

    @objc private func scrolled(_ g: UIPanGestureRecognizer) {
        guard g.numberOfTouches == 0 else { return }
        let t = g.translation(in: metalView)
        let phase: Int32
        switch g.state {
        case .began:
            lastScroll = .zero
            phase = 0
        case .changed: phase = 1
        case .ended: phase = 2
        default: phase = 3
        }
        let d = CGPoint(x: t.x - lastScroll.x, y: t.y - lastScroll.y)
        lastScroll = t
        let p = g.location(in: metalView)
        send { host in
            fc_host_hover(host, Float(p.x), Float(p.y), true)
            fc_host_scroll(host, Float(d.x), Float(d.y), phase)
        }
    }

    @objc private func pinched(_ g: UIPinchGestureRecognizer) {
        if g.state == .began { lastPinch = 1 }
        guard g.state == .began || g.state == .changed, lastPinch > 0 else { return }
        let f = g.scale / lastPinch
        lastPinch = g.scale
        let p = g.location(in: metalView)
        send { fc_host_pinch($0, Float(f), Float(p.x), Float(p.y)) }
    }

    // MARK: - Hardware keyboard

    private func forward(presses: Set<UIPress>, pressed: Bool) -> Bool {
        guard let host else { return false }
        var handled = false
        let typing = keyboardProxy.isFirstResponder
        for press in presses {
            guard let key = press.key else { continue }
            let usage = UInt32(key.keyCode.rawValue)
            let flags = UInt64(key.modifierFlags.rawValue)
            // while the on-screen keyboard's field has focus, UIKit types the text through it
            if typing && [40, 42, 43, 88].contains(usage) { handled = true; continue }
            if pressed && key.keyCode == .keyboardV && key.modifierFlags.contains(.command) && !key.modifierFlags.contains(.shift) {
                let text = UIPasteboard.general.string ?? ""
                text.withCString { fc_host_paste(host, $0) }
            }
            let chars = typing ? "" : key.characters
            chars.withCString { c in
                key.charactersIgnoringModifiers.withCString { u in
                    fc_host_key(host, usage, pressed, false, flags, c, u)
                }
            }
            handleRepeat(usage: usage, flags: flags, unmodified: key.charactersIgnoringModifiers, pressed: pressed)
            handled = true
        }
        requestFrame(after: 0)
        return handled
    }

    /// UIKit sends no key repeats: arrows (frame stepping) and delete repeat while held.
    private func handleRepeat(usage: UInt32, flags: UInt64, unmodified: String, pressed: Bool) {
        if !pressed {
            repeatTimers.removeValue(forKey: usage)?.invalidate()
            return
        }
        repeatTimers.removeValue(forKey: usage)?.invalidate()
        guard [42, 76, 79, 80, 81, 82].contains(usage) else { return }
        let start = Timer(timeInterval: 0.42, repeats: false) { [weak self] _ in
            guard let self, self.repeatTimers[usage] != nil else { return }
            let repeatTimer = Timer(timeInterval: 1.0 / 24.0, repeats: true) { [weak self] _ in
                self?.send { host in
                    "".withCString { c in unmodified.withCString { u in fc_host_key(host, usage, true, true, flags, c, u) } }
                }
            }
            RunLoop.main.add(repeatTimer, forMode: .common)
            self.repeatTimers[usage] = repeatTimer
        }
        RunLoop.main.add(start, forMode: .common)
        repeatTimers[usage] = start
    }

    private func stopAllRepeatTimers() {
        for timer in repeatTimers.values { timer.invalidate() }
        repeatTimers.removeAll(keepingCapacity: true)
    }

    override func pressesBegan(_ presses: Set<UIPress>, with event: UIPressesEvent?) {
        if !forward(presses: presses, pressed: true) { super.pressesBegan(presses, with: event) }
    }

    override func pressesEnded(_ presses: Set<UIPress>, with event: UIPressesEvent?) {
        if !forward(presses: presses, pressed: false) { super.pressesEnded(presses, with: event) }
    }

    override func pressesCancelled(_ presses: Set<UIPress>, with event: UIPressesEvent?) {
        if !forward(presses: presses, pressed: false) { super.pressesCancelled(presses, with: event) }
    }
}
