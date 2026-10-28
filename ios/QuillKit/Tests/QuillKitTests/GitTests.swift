import Foundation
import Testing

@testable import QuillKit

/// Run `git` with `args` in `dir` (the system git stands in for the desktop).
@discardableResult
func git(_ args: String..., in dir: URL) throws -> String {
    let process = Process()
    process.executableURL = URL(fileURLWithPath: "/usr/bin/env")
    process.arguments = ["git", "-c", "user.name=Desk", "-c", "user.email=desk@example.com"] + args
    process.currentDirectoryURL = dir
    let out = Pipe()
    process.standardOutput = out
    process.standardError = out
    try process.run()
    process.waitUntilExit()
    let text = String(data: out.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8) ?? ""
    guard process.terminationStatus == 0 else {
        throw QuillError.Io(message: "git \(args.joined(separator: " ")) failed: \(text)")
    }
    return text
}

@MainActor
@Suite struct GitTests {
    @Test func pullBringsInTheDesktopsEdits() async throws {
        let root = try scratch("pull")
        let remote = root.appendingPathComponent("remote.git")
        let desk = root.appendingPathComponent("desk")
        let phone = root.appendingPathComponent("phone")
        try FileManager.default.createDirectory(at: desk, withIntermediateDirectories: true)
        try git("init", "--bare", remote.path, in: root)
        try writeNovel(in: desk)
        try git("init", in: desk)
        try git("add", ".", in: desk)
        try git("commit", "-m", "start", in: desk)
        try git("push", remote.path, "HEAD:refs/heads/main", in: desk)
        try git("clone", "--branch", "main", remote.path, phone.path, in: root)

        let store = ProjectStore()
        store.open(phone)
        #expect(store.chapters.map(\.words) == [6, 3])

        try "Wind. Then rain. Then the sea came over the wall.\n"
            .write(to: desk.appendingPathComponent("storm.md"), atomically: true, encoding: .utf8)
        try git("commit", "-am", "storm", in: desk)
        try git("push", remote.path, "HEAD:refs/heads/main", in: desk)

        let message = try await store.pull()
        #expect(message.contains("Pulled"))
        #expect(store.chapters.map(\.words) == [6, 10])
        #expect(store.remote == remote.path)
    }

    @Test func cloneRefusesANonHttpsAddressAndKeepsNoToken() async throws {
        let root = try scratch("clone-ssh")
        let tokens = MemoryTokenStore()
        let library = Library(folder: root, tokens: tokens)
        await #expect(throws: QuillError.self) {
            _ = try await library.clone(url: "git@github.com:me/novel.git", token: "t0ken")
        }
        #expect(tokens.token(forHost: "github.com") == nil)
        #expect(library.projects.isEmpty)
    }

    @Test func aFailedCloneLeavesNoFolderOrToken() async throws {
        let root = try scratch("clone-fail")
        let tokens = MemoryTokenStore()
        let library = Library(folder: root, tokens: tokens)
        // Nothing listens on port 9 of localhost: the clone fails fast.
        await #expect(throws: QuillError.self) {
            _ = try await library.clone(url: "https://127.0.0.1:9/me/novel.git", token: "t0ken")
        }
        #expect(tokens.token(forHost: "127.0.0.1") == nil)
        let left = try FileManager.default.contentsOfDirectory(atPath: root.path)
        #expect(left.isEmpty, "left behind: \(left)")
    }

    @Test func theLibraryListsProjectFolders() throws {
        let root = try scratch("library")
        let novel = root.appendingPathComponent("Novel")
        try FileManager.default.createDirectory(at: novel, withIntermediateDirectories: true)
        try writeNovel(in: novel)
        try FileManager.default.createDirectory(
            at: root.appendingPathComponent("NotAProject"), withIntermediateDirectories: true)
        let library = Library(folder: root, tokens: MemoryTokenStore())
        #expect(library.projects.map(\.name) == ["Novel"])
    }

    @Test func tokensAreKeptPerHost() throws {
        #expect(tokenHost(for: "https://GitHub.com/me/novel.git") == "github.com")
        #expect(tokenHost(for: "not a url") == nil)
    }
}
