// FilmCraft's file dialogs on iPadOS: Photos and Files pickers for import, a name prompt for saving
// (projects in Documents/Projects, exports in Documents/Exports, all visible in the Files app).
//
// Media chosen outside the app (Photos, iCloud Drive, a USB drive) is copied into Documents/Media,
// so a project always opens: iPadOS gives an app no lasting path to files it does not own.

import PhotosUI
import UIKit
import UniformTypeIdentifiers

final class Dialogs: NSObject, UIDocumentPickerDelegate, PHPickerViewControllerDelegate, UIAdaptivePresentationControllerDelegate {
    weak var controller: FilmCraftViewController?
    /// The request being answered, and where its files go.
    private var request: UInt64 = 0
    private var destination = "Media"
    private var keepAccess = false
    private var overlay: UIView?

    static var documents: URL { FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0] }

    static func json(_ paths: [String]) -> String {
        (try? JSONSerialization.data(withJSONObject: paths)).flatMap { String(data: $0, encoding: .utf8) } ?? "[]"
    }

    /// Answer the pending request (exactly once).
    private func answer(_ paths: [String]) {
        let id = request
        request = 0
        guard id != 0 else { return }
        Dialogs.json(paths).withCString { fc_dialog_answer(id, $0) }
        controller?.requestFrame(after: 0)
    }

    // MARK: - Requests from Rust

    func present(request id: UInt64, json: String) {
        if request != 0 { answer([]) }
        request = id
        guard let controller,
              let data = json.data(using: .utf8),
              let spec = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
              let kind = spec["kind"] as? String
        else { return answer([]) }
        let exts = (spec["exts"] as? [String]) ?? []
        let name = (spec["name"] as? String) ?? "Sans titre"
        switch kind {
        case "importMedia":
            chooseImportSource(from: controller)
        case "openFile":
            pickFiles(types: Dialogs.types(exts), multiple: false, to: "Imports", from: controller)
        case "relink":
            pickFiles(types: Dialogs.types(exts), multiple: false, to: "Media", from: controller)
        case "openProject":
            pickFiles(types: Dialogs.types(["fcproj"]), multiple: false, to: "Projects", from: controller, startIn: Dialogs.documents.appendingPathComponent("Projects"))
        case "saveProject":
            askName(title: "Enregistrer le projet", suggested: name, ext: "fcproj", folder: "Projects", from: controller)
        case "saveAs":
            askName(title: "Exporter", suggested: name, ext: exts.first ?? "", folder: "Exports", from: controller)
        case "folder":
            pickFolder(from: controller)
        default:
            answer([])
        }
    }

    static func types(_ exts: [String]) -> [UTType] {
        let t = exts.compactMap { UTType(filenameExtension: $0) }
        return t.isEmpty ? [.item] : t
    }

    // MARK: - Import: Photos or Files

    private func chooseImportSource(from vc: UIViewController) {
        let sheet = UIAlertController(title: "Importer des médias", message: nil, preferredStyle: .actionSheet)
        sheet.addAction(UIAlertAction(title: "Photos", style: .default) { [weak self] _ in self?.pickPhotos(from: vc) })
        sheet.addAction(UIAlertAction(title: "Fichiers", style: .default) { [weak self] _ in
            // Let FilmCraft validate the selected extension. The Files picker must also expose
            // subtitles, LUTs and interchange files (.srt/.vtt/.cube/.edl/.xml), which are valid
            // project inputs even though they are not audiovisual UTTypes.
            self?.pickFiles(types: [.item], multiple: true, to: "Media", from: vc)
        })
        sheet.addAction(UIAlertAction(title: "Annuler", style: .cancel) { [weak self] _ in self?.answer([]) })
        anchor(sheet, in: vc)
        vc.present(sheet, animated: true)
    }

    private func pickPhotos(from vc: UIViewController) {
        var config = PHPickerConfiguration()
        config.filter = .any(of: [.videos, .images])
        config.selectionLimit = 0
        // the original file (HEVC, ProRes, HDR) rather than a converted copy
        config.preferredAssetRepresentationMode = .current
        let picker = PHPickerViewController(configuration: config)
        picker.delegate = self
        picker.presentationController?.delegate = self
        vc.present(picker, animated: true)
    }

    func picker(_ picker: PHPickerViewController, didFinishPicking results: [PHPickerResult]) {
        picker.dismiss(animated: true)
        guard !results.isEmpty else { return answer([]) }
        showOverlay("Import depuis Photos…")
        let folder = Dialogs.folder("Media")
        let group = DispatchGroup()
        let lock = NSLock()
        var paths = [(Int, String)]()
        for (i, r) in results.enumerated() {
            let provider = r.itemProvider
            let type = [UTType.movie, .image].first { provider.hasItemConformingToTypeIdentifier($0.identifier) }
            guard let type else { continue }
            group.enter()
            provider.loadFileRepresentation(forTypeIdentifier: type.identifier) { url, _ in
                defer { group.leave() }
                // the file exists only during this call: copy it now
                guard let url else { return }
                let base = provider.suggestedName ?? url.deletingPathExtension().lastPathComponent
                let target = Dialogs.unique(folder.appendingPathComponent(base).appendingPathExtension(url.pathExtension))
                do {
                    try FileManager.default.copyItem(at: url, to: target)
                    lock.lock()
                    paths.append((i, target.path))
                    lock.unlock()
                } catch {
                    NSLog("FilmCraft: Photos import: \(error)")
                }
            }
        }
        group.notify(queue: .main) { [weak self] in
            self?.hideOverlay()
            self?.answer(paths.sorted { $0.0 < $1.0 }.map(\.1))
        }
    }

    private func pickFiles(types: [UTType], multiple: Bool, to folder: String, from vc: UIViewController, startIn: URL? = nil) {
        destination = folder
        keepAccess = false
        let picker = UIDocumentPickerViewController(forOpeningContentTypes: types, asCopy: false)
        picker.allowsMultipleSelection = multiple
        picker.directoryURL = startIn
        picker.delegate = self
        vc.present(picker, animated: true)
    }

    private func pickFolder(from vc: UIViewController) {
        keepAccess = true
        let picker = UIDocumentPickerViewController(forOpeningContentTypes: [.folder], asCopy: false)
        picker.delegate = self
        vc.present(picker, animated: true)
    }

    func documentPicker(_ picker: UIDocumentPickerViewController, didPickDocumentsAt urls: [URL]) {
        if keepAccess {
            // a folder chosen for FilmCraft to write into: keep the permission for this session
            let paths = urls.filter { $0.startAccessingSecurityScopedResource() || FileManager.default.isWritableFile(atPath: $0.path) }.map(\.path)
            return answer(paths)
        }
        guard let vc = controller else { return answer([]) }
        Dialogs.importCopies(of: urls, into: destination, from: vc, dialogs: self) { [weak self] paths in self?.answer(paths) }
    }

    func documentPickerWasCancelled(_ picker: UIDocumentPickerViewController) {
        answer([])
    }

    /// The Photos picker swiped away without choosing.
    func presentationControllerDidDismiss(_ presentationController: UIPresentationController) {
        answer([])
    }

    // MARK: - Copies into the app

    static func folder(_ name: String) -> URL {
        let url = documents.appendingPathComponent(name, isDirectory: true)
        try? FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        return url
    }

    /// `url`, or `name 2.ext`, `name 3.ext`… when it exists.
    static func unique(_ url: URL) -> URL {
        let fm = FileManager.default
        guard fm.fileExists(atPath: url.path) else { return url }
        let dir = url.deletingLastPathComponent()
        let base = url.deletingPathExtension().lastPathComponent
        let ext = url.pathExtension
        for n in 2...9999 {
            let candidate = dir.appendingPathComponent("\(base) \(n)").appendingPathExtension(ext)
            if !fm.fileExists(atPath: candidate.path) { return candidate }
        }
        return dir.appendingPathComponent(UUID().uuidString).appendingPathExtension(ext)
    }

    /// Paths FilmCraft can keep using for `urls`: files already inside the app as they are, others
    /// copied into Documents/`folder` (projects into Projects). Copies run off the main thread.
    static func importCopies(of urls: [URL], into folder: String = "Media", from vc: FilmCraftViewController, dialogs: Dialogs? = nil, completion: @escaping ([String]) -> Void) {
        let docs = documents.standardizedFileURL.resolvingSymlinksInPath().path
        let owner = dialogs ?? vc.dialogs
        owner.showOverlay("Import en cours…")
        DispatchQueue.global(qos: .userInitiated).async {
            var paths = [String]()
            for url in urls {
                let resolved = url.standardizedFileURL.resolvingSymlinksInPath().path
                if resolved.hasPrefix(docs + "/") {
                    paths.append(resolved)
                    continue
                }
                let scoped = url.startAccessingSecurityScopedResource()
                defer { if scoped { url.stopAccessingSecurityScopedResource() } }
                let into = url.pathExtension.lowercased() == "fcproj" ? "Projects" : folder
                let target = unique(Dialogs.folder(into).appendingPathComponent(url.lastPathComponent))
                var copyError: NSError?
                var failure: Error?
                // coordinated: iCloud Drive and other providers download the file first
                NSFileCoordinator().coordinate(readingItemAt: url, options: [.withoutChanges], error: &copyError) { readable in
                    do { try FileManager.default.copyItem(at: readable, to: target) } catch { failure = error }
                }
                if let e = copyError ?? failure {
                    NSLog("FilmCraft: import \(url.lastPathComponent): \(e)")
                } else {
                    paths.append(target.path)
                }
            }
            DispatchQueue.main.async {
                owner.hideOverlay()
                completion(paths)
            }
        }
    }

    // MARK: - Save: a name, in Documents

    private func askName(title: String, suggested: String, ext: String, folder: String, from vc: UIViewController) {
        var base = (suggested as NSString).lastPathComponent
        if !ext.isEmpty, (base as NSString).pathExtension.lowercased() == ext.lowercased() {
            base = (base as NSString).deletingPathExtension
        }
        let alert = UIAlertController(title: title, message: "Dans Fichiers ▸ Sur mon iPad ▸ FilmCraft ▸ \(folder == "Projects" ? "Projects" : "Exports")", preferredStyle: .alert)
        alert.addTextField { f in
            f.text = base
            f.clearButtonMode = .whileEditing
            f.autocorrectionType = .no
        }
        alert.addAction(UIAlertAction(title: "Annuler", style: .cancel) { [weak self] _ in self?.answer([]) })
        alert.addAction(UIAlertAction(title: "Enregistrer", style: .default) { [weak self, weak alert] _ in
            guard let self else { return }
            let raw = alert?.textFields?.first?.text?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
            let clean = raw.replacingOccurrences(of: "/", with: "-").replacingOccurrences(of: ":", with: "-")
            let name = clean.isEmpty ? "Sans titre" : clean
            var url = Dialogs.folder(folder).appendingPathComponent(name)
            if !ext.isEmpty { url = url.appendingPathExtension(ext) }
            if FileManager.default.fileExists(atPath: url.path) {
                self.confirmReplace(url, from: vc)
            } else {
                self.answer([url.path])
            }
        })
        vc.present(alert, animated: true)
    }

    private func confirmReplace(_ url: URL, from vc: UIViewController) {
        let alert = UIAlertController(title: "« \(url.lastPathComponent) » existe déjà", message: "Le remplacer ?", preferredStyle: .alert)
        alert.addAction(UIAlertAction(title: "Annuler", style: .cancel) { [weak self] _ in self?.answer([]) })
        alert.addAction(UIAlertAction(title: "Remplacer", style: .destructive) { [weak self] _ in self?.answer([url.path]) })
        vc.present(alert, animated: true)
    }

    // MARK: - Progress overlay and popovers

    func showOverlay(_ text: String) {
        guard overlay == nil, let view = controller?.view else { return }
        let box = UIView(frame: view.bounds)
        box.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        box.backgroundColor = UIColor(white: 0, alpha: 0.45)
        let spinner = UIActivityIndicatorView(style: .large)
        spinner.color = .white
        spinner.startAnimating()
        let label = UILabel()
        label.text = text
        label.textColor = .white
        label.font = .systemFont(ofSize: 15, weight: .medium)
        let stack = UIStackView(arrangedSubviews: [spinner, label])
        stack.axis = .vertical
        stack.spacing = 12
        stack.alignment = .center
        stack.translatesAutoresizingMaskIntoConstraints = false
        box.addSubview(stack)
        stack.centerXAnchor.constraint(equalTo: box.centerXAnchor).isActive = true
        stack.centerYAnchor.constraint(equalTo: box.centerYAnchor).isActive = true
        view.addSubview(box)
        overlay = box
    }

    func hideOverlay() {
        overlay?.removeFromSuperview()
        overlay = nil
    }

    func anchor(_ alert: UIViewController, in vc: UIViewController) {
        if let pop = alert.popoverPresentationController {
            pop.sourceView = vc.view
            pop.sourceRect = CGRect(x: vc.view.bounds.midX, y: vc.view.bounds.midY, width: 1, height: 1)
            pop.permittedArrowDirections = []
        }
    }
}
