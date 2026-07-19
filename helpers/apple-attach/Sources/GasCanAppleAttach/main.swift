import ContainerAPIClient
import Foundation

func diagnostic(_ message: String) {
    guard ProcessInfo.processInfo.environment["GASCAN_ATTACH_DIAGNOSTICS"] != nil else {
        return
    }
    try? FileHandle.standardError.write(contentsOf: Data("gascan-apple-attach: \(message)\n".utf8))
}

@discardableResult
func drainAfterWait(
    relays: [OutputRelay],
    timeout: Duration
) async -> Bool {
    await withTaskGroup(of: Bool.self, returning: Bool.self) { group in
        group.addTask {
            for relay in relays {
                _ = try? await relay.task.value
            }
            return true
        }
        group.addTask {
            try? await Task.sleep(for: timeout)
            return false
        }

        let drained = await group.next() ?? false
        if !drained {
            relays.forEach { $0.cancel() }
        }
        group.cancelAll()
        return drained
    }
}

final class RelaySource: @unchecked Sendable {
    private let handle: FileHandle
    private let continuation: AsyncStream<Data>.Continuation

    init(handle: FileHandle, continuation: AsyncStream<Data>.Continuation) {
        self.handle = handle
        self.continuation = continuation
    }

    func arm() {
        handle.readabilityHandler = { [weak self] readable in
            guard let self else {
                return
            }
            readable.readabilityHandler = nil
            let data = readable.availableData
            if data.isEmpty {
                continuation.finish()
            } else {
                continuation.yield(data)
            }
        }
    }

    func stop() {
        handle.readabilityHandler = nil
        continuation.finish()
        try? handle.close()
    }
}

struct OutputRelay: Sendable {
    let task: Task<Void, any Error>

    init(
        handle: FileHandle,
        consume: @escaping @Sendable (Data) async throws -> Void
    ) {
        let (stream, continuation) = AsyncStream<Data>.makeStream(
            bufferingPolicy: .bufferingOldest(1)
        )
        let source = RelaySource(handle: handle, continuation: continuation)
        continuation.onTermination = { _ in
            source.stop()
        }
        source.arm()
        task = Task {
            defer {
                source.stop()
            }
            for await data in stream {
                try Task.checkCancellation()
                try await consume(data)
                source.arm()
            }
        }
    }

    func cancel() {
        task.cancel()
    }
}

actor FrameEmitter {
    private var terminal = false
    private let encoder = JSONEncoder()

    func emit(_ frame: OutputFrame) throws {
        guard !terminal else {
            return
        }
        if case .error = frame {
            terminal = true
        }
        if case .exit = frame {
            terminal = true
        }
        var data = try encoder.encode(frame)
        data.append(0x0a)
        try FileHandle.standardOutput.write(contentsOf: data)
    }
}

private enum SessionTaskEvent: @unchecked Sendable {
    case input(Result<Void, any Error>)
    case process(Result<Int32, any Error>)
}

func superviseSession(
    inputTask: Task<Void, any Error>,
    waitTask: Task<Int32, any Error>
) async throws -> Int32 {
    let (events, continuation) = AsyncStream<SessionTaskEvent>.makeStream(
        bufferingPolicy: .bufferingNewest(2)
    )
    let inputObserver = Task {
        continuation.yield(.input(await inputTask.result))
    }
    let waitObserver = Task {
        continuation.yield(.process(await waitTask.result))
    }
    defer {
        continuation.finish()
        inputObserver.cancel()
        waitObserver.cancel()
    }

    for await event in events {
        switch event {
        case .input(.success):
            continue
        case .input(.failure(let error)):
            waitTask.cancel()
            throw error
        case .process(.success(let code)):
            inputTask.cancel()
            return code
        case .process(.failure(let error)):
            inputTask.cancel()
            throw error
        }
    }
    throw CancellationError()
}

struct GasCanAppleAttach {
    static func runMain() async {
        let emitter = FrameEmitter()
        do {
            try await run(emitter: emitter)
        } catch let failure as ProtocolFailure {
            try? await emitter.emit(.error(code: "protocol", message: String(describing: failure)))
        } catch {
            try? await emitter.emit(.error(code: "apple_api", message: String(describing: error)))
        }
    }

    private static func run(emitter: FrameEmitter) async throws {
        var inputLines = FileHandle.standardInput.bytes.lines.makeAsyncIterator()
        guard let firstLine = try await inputLines.next() else {
            throw ProtocolFailure.expectedStart
        }
        let first = try JSONDecoder().decode(InputFrame.self, from: Data(firstLine.utf8))
        try validateVersion(first)
        diagnostic("validated start frame")
        guard case .start(_, let containerID, let argv, let tty, let environment) = first else {
            throw ProtocolFailure.expectedStart
        }
        guard let executable = argv.first else {
            throw ProtocolFailure.emptyArgv
        }

        let inputPipe = Pipe()
        let outputPipe = Pipe()
        let errorPipe = tty ? nil : Pipe()
        let client = ContainerClient()
        let container = try await client.get(id: containerID)
        diagnostic("resolved container")
        var configuration = container.configuration.initProcess
        configuration.executable = executable
        configuration.arguments = Array(argv.dropFirst())
        configuration.terminal = tty
        configuration.environment = try overlayEnvironment(
            configuration.environment,
            with: environment
        )

        let process = try await client.createProcess(
            containerId: container.id,
            processId: UUID().uuidString.lowercased(),
            configuration: configuration,
            stdio: [
                inputPipe.fileHandleForReading,
                outputPipe.fileHandleForWriting,
                errorPipe?.fileHandleForWriting,
            ]
        )
        diagnostic("created guest process \(process.id)")
        try await process.start()
        diagnostic("started guest process")
        try inputPipe.fileHandleForReading.close()
        try outputPipe.fileHandleForWriting.close()
        try errorPipe?.fileHandleForWriting.close()

        let stdoutRelay = OutputRelay(handle: outputPipe.fileHandleForReading) { data in
            try await emitter.emit(.stdout(data))
        }
        let stderrRelay = errorPipe.map { pipe in
            OutputRelay(handle: pipe.fileHandleForReading) { data in
                try await emitter.emit(.stderr(data))
            }
        }
        let inputTask = Task<Void, any Error> {
            try await handleInputs(
                process: process,
                input: inputPipe.fileHandleForWriting,
                lines: &inputLines
            )
        }
        let waitTask = Task<Int32, any Error> {
            try await process.wait()
        }
        let code: Int32
        do {
            code = try await superviseSession(inputTask: inputTask, waitTask: waitTask)
        } catch {
            inputTask.cancel()
            waitTask.cancel()
            stdoutRelay.cancel()
            stderrRelay?.cancel()
            try? inputPipe.fileHandleForWriting.close()
            throw error
        }
        diagnostic("guest wait returned \(code); draining output")
        try? inputPipe.fileHandleForWriting.close()
        var relays = [stdoutRelay]
        if let stderrRelay {
            relays.append(stderrRelay)
        }
        let drained = await drainAfterWait(
            relays: relays,
            timeout: .seconds(3)
        )
        if !drained {
            diagnostic("output drain timed out; readers cancelled and closed")
        }
        diagnostic("output drain completed; emitting exit")
        try await emitter.emit(.exit(code))
    }

    private static func handleInputs(
        process: ClientProcess,
        input: FileHandle,
        lines: inout AsyncLineSequence<FileHandle.AsyncBytes>.AsyncIterator
    ) async throws {
        while let line = try await lines.next() {
            try Task.checkCancellation()
            let frame = try JSONDecoder().decode(InputFrame.self, from: Data(line.utf8))
            try validateVersion(frame)
            switch frame {
            case .start:
                throw ProtocolFailure.duplicateStart
            case .stdin(_, let data):
                try input.write(contentsOf: data)
            case .resize(_, let rows, let cols):
                try await process.resize(.init(width: cols, height: rows))
            case .signal(_, let signal):
                try validateSignal(signal)
            case .close:
                try input.close()
                return
            }
        }
        try? input.close()
    }

}

await GasCanAppleAttach.runMain()
