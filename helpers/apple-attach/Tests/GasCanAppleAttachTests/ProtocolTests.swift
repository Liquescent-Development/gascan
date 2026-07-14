import Foundation
import Testing
@testable import GasCanAppleAttach

@Test func protocolDecodesBase64WithoutLosingBytes() throws {
    let frame = try JSONDecoder().decode(
        InputFrame.self,
        from: Data(#"{"version":1,"type":"stdin","data":"AP8="}"#.utf8)
    )
    #expect(frame == .stdin(version: 1, data: Data([0, 255])))
}

@Test func protocolEncodesTypedTerminalFrames() throws {
    let exit = try JSONSerialization.jsonObject(with: JSONEncoder().encode(OutputFrame.exit(42)))
    #expect((exit as? [String: Any])?["version"] as? Int == 1)
    #expect((exit as? [String: Any])?["type"] as? String == "exit")
    #expect((exit as? [String: Any])?["code"] as? Int == 42)

    let error = try JSONSerialization.jsonObject(
        with: JSONEncoder().encode(OutputFrame.error(code: "bad_frame", message: "invalid"))
    )
    #expect((error as? [String: Any])?["type"] as? String == "error")
    #expect((error as? [String: Any])?["code"] as? String == "bad_frame")
}

@Test func versionAndSignalRejectionAreStrict() throws {
    #expect(throws: ProtocolFailure.self) {
        try validateVersion(.close(version: 2))
    }
    #expect(throws: ProtocolFailure.self) {
        try validateSignal(SIGINT)
    }
    #expect(throws: ProtocolFailure.self) {
        try validateSignal(SIGTERM)
    }
    #expect(throws: ProtocolFailure.self) {
        try validateSignal(SIGKILL)
    }
}

private actor ReceivedBytes {
    private(set) var bytes: [UInt8] = []

    func append(_ byte: UInt8) {
        bytes.append(byte)
    }
}

@Test func boundedDrainPreservesBytesWhenReaderNeverReachesEOF() async throws {
    let pipe = Pipe()
    let received = ReceivedBytes()
    let reader = pipe.fileHandleForReading
    let relay = OutputRelay(handle: reader) { data in
        for byte in data {
            await received.append(byte)
        }
    }
    try pipe.fileHandleForWriting.write(contentsOf: Data([0, 255]))

    while await received.bytes.count < 2 {
        await Task.yield()
    }
    let before = ContinuousClock.now
    await drainAfterWait(
        relays: [relay],
        timeout: .milliseconds(50)
    )
    let elapsed = before.duration(to: .now)

    #expect(await received.bytes == [0, 255])
    #expect(elapsed < .seconds(1))
    try? pipe.fileHandleForWriting.close()
}

private actor CompletionFlag {
    private(set) var value = false

    func set() {
        value = true
    }
}

@Test func structuredDrainReturnsWhenReaderIgnoresCancellation() async throws {
    let pipe = Pipe()
    let completed = CompletionFlag()
    let relay = OutputRelay(handle: pipe.fileHandleForReading) { _ in
    }

    let drainTask = Task {
        await drainAfterWait(relays: [relay], timeout: .milliseconds(50))
        await completed.set()
    }
    try await Task.sleep(for: .milliseconds(150))
    let returnedWithinBound = await completed.value

    await drainTask.value
    #expect(returnedWithinBound)
    try? pipe.fileHandleForWriting.close()
}

@Test func inputFailureWinsAndCancelsProcessWait() async throws {
    let waitCancelled = CompletionFlag()
    let inputTask = Task<Void, any Error> {
        throw ProtocolFailure.duplicateStart
    }
    let waitTask = Task<Int32, any Error> {
        defer { Task { await waitCancelled.set() } }
        try await Task.sleep(for: .seconds(30))
        return 0
    }

    await #expect(throws: ProtocolFailure.self) {
        try await superviseSession(inputTask: inputTask, waitTask: waitTask)
    }
    while !(await waitCancelled.value) {
        await Task.yield()
    }
}

@Test func normalInputCompletionDoesNotPreemptExactExit() async throws {
    let inputTask = Task<Void, any Error> {}
    let waitTask = Task<Int32, any Error> {
        try await Task.sleep(for: .milliseconds(20))
        return 42
    }

    let code = try await superviseSession(inputTask: inputTask, waitTask: waitTask)
    #expect(code == 42)
}
