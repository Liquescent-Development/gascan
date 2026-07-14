import Foundation

let protocolVersion: UInt32 = 1

enum InputFrame: Equatable {
    case start(version: UInt32, container: String, argv: [String], tty: Bool)
    case stdin(version: UInt32, data: Data)
    case resize(version: UInt32, rows: UInt16, cols: UInt16)
    case signal(version: UInt32, signal: Int32)
    case close(version: UInt32)

    var version: UInt32 {
        switch self {
        case .start(let version, _, _, _), .stdin(let version, _),
             .resize(let version, _, _), .signal(let version, _), .close(let version):
            version
        }
    }
}

extension InputFrame: Codable {
    private enum Keys: String, CodingKey {
        case version, type, container, argv, tty, data, rows, cols, signal
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: Keys.self)
        let version = try values.decode(UInt32.self, forKey: .version)
        switch try values.decode(String.self, forKey: .type) {
        case "start":
            self = .start(
                version: version,
                container: try values.decode(String.self, forKey: .container),
                argv: try values.decode([String].self, forKey: .argv),
                tty: try values.decode(Bool.self, forKey: .tty)
            )
        case "stdin":
            self = .stdin(version: version, data: try values.decode(Data.self, forKey: .data))
        case "resize":
            self = .resize(
                version: version,
                rows: try values.decode(UInt16.self, forKey: .rows),
                cols: try values.decode(UInt16.self, forKey: .cols)
            )
        case "signal":
            self = .signal(version: version, signal: try values.decode(Int32.self, forKey: .signal))
        case "close":
            self = .close(version: version)
        case let type:
            throw DecodingError.dataCorruptedError(
                forKey: .type,
                in: values,
                debugDescription: "unknown frame type \(type)"
            )
        }
    }

    func encode(to encoder: Encoder) throws {
        var values = encoder.container(keyedBy: Keys.self)
        try values.encode(version, forKey: .version)
        switch self {
        case .start(_, let container, let argv, let tty):
            try values.encode("start", forKey: .type)
            try values.encode(container, forKey: .container)
            try values.encode(argv, forKey: .argv)
            try values.encode(tty, forKey: .tty)
        case .stdin(_, let data):
            try values.encode("stdin", forKey: .type)
            try values.encode(data, forKey: .data)
        case .resize(_, let rows, let cols):
            try values.encode("resize", forKey: .type)
            try values.encode(rows, forKey: .rows)
            try values.encode(cols, forKey: .cols)
        case .signal(_, let signal):
            try values.encode("signal", forKey: .type)
            try values.encode(signal, forKey: .signal)
        case .close:
            try values.encode("close", forKey: .type)
        }
    }
}

enum OutputFrame: Encodable, Equatable {
    case stdout(Data)
    case stderr(Data)
    case error(code: String, message: String)
    case exit(Int32)

    private enum Keys: String, CodingKey {
        case version, type, data, code, message
    }

    func encode(to encoder: Encoder) throws {
        var values = encoder.container(keyedBy: Keys.self)
        try values.encode(protocolVersion, forKey: .version)
        switch self {
        case .stdout(let data):
            try values.encode("stdout", forKey: .type)
            try values.encode(data, forKey: .data)
        case .stderr(let data):
            try values.encode("stderr", forKey: .type)
            try values.encode(data, forKey: .data)
        case .error(let code, let message):
            try values.encode("error", forKey: .type)
            try values.encode(code, forKey: .code)
            try values.encode(message, forKey: .message)
        case .exit(let code):
            try values.encode("exit", forKey: .type)
            try values.encode(code, forKey: .code)
        }
    }
}

enum ProtocolFailure: Error {
    case invalidVersion(UInt32)
    case expectedStart
    case duplicateStart
    case emptyArgv
    case invalidSignal(Int32)
}

func validateVersion(_ frame: InputFrame) throws {
    guard frame.version == protocolVersion else {
        throw ProtocolFailure.invalidVersion(frame.version)
    }
}

func validateSignal(_ signal: Int32) throws {
    throw ProtocolFailure.invalidSignal(signal)
}
