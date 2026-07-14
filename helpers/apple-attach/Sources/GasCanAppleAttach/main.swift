import ContainerAPIClient
import Foundation

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
        guard case .start(_, let containerID, let argv, let tty) = first else {
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
        var configuration = container.configuration.initProcess
        configuration.executable = executable
        configuration.arguments = Array(argv.dropFirst())
        configuration.terminal = tty

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
        try await process.start()
        try inputPipe.fileHandleForReading.close()
        try outputPipe.fileHandleForWriting.close()
        try errorPipe?.fileHandleForWriting.close()

        let stdoutTask = Task {
            try await forward(
                outputPipe.fileHandleForReading,
                frame: OutputFrame.stdout,
                emitter: emitter
            )
        }
        let stderrTask = Task {
            if let errorPipe {
                try await forward(
                    errorPipe.fileHandleForReading,
                    frame: OutputFrame.stderr,
                    emitter: emitter
                )
            }
        }
        let inputTask = Task {
            try await handleInputs(
                process: process,
                input: inputPipe.fileHandleForWriting,
                lines: &inputLines
            )
        }

        let code = try await process.wait()
        inputTask.cancel()
        try? inputPipe.fileHandleForWriting.close()
        try await stdoutTask.value
        try await stderrTask.value
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
                try await process.kill(signal)
            case .close:
                try input.close()
                return
            }
        }
        try? input.close()
    }

    private static func forward(
        _ input: FileHandle,
        frame: @escaping @Sendable (Data) -> OutputFrame,
        emitter: FrameEmitter
    ) async throws {
        for try await byte in input.bytes {
            try Task.checkCancellation()
            try await emitter.emit(frame(Data([byte])))
        }
    }
}

await GasCanAppleAttach.runMain()
