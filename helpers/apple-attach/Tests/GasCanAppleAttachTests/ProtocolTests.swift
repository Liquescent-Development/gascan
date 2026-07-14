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

@Test func versionAndSignalAllowlistAreStrict() throws {
    #expect(throws: ProtocolFailure.self) {
        try validateVersion(.close(version: 2))
    }
    try validateSignal(SIGINT)
    try validateSignal(SIGTERM)
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
    let task = Task {
        for try await byte in reader.bytes {
            await received.append(byte)
        }
    }
    try pipe.fileHandleForWriting.write(contentsOf: Data([0, 255]))

    while await received.bytes.count < 2 {
        await Task.yield()
    }
    let before = ContinuousClock.now
    await drainAfterWait(
        tasks: [task],
        handles: [reader],
        timeout: .milliseconds(50)
    )
    let elapsed = before.duration(to: .now)

    #expect(await received.bytes == [0, 255])
    #expect(elapsed < .seconds(1))
    try? pipe.fileHandleForWriting.close()
}
