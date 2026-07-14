// swift-tools-version: 6.2

import PackageDescription

let package = Package(
    name: "gascan-apple-attach",
    platforms: [.macOS(.v15)],
    products: [
        .executable(name: "gascan-apple-attach", targets: ["GasCanAppleAttach"])
    ],
    dependencies: [
        .package(url: "https://github.com/apple/container.git", exact: "1.1.0")
    ],
    targets: [
        .executableTarget(
            name: "GasCanAppleAttach",
            dependencies: [
                .product(name: "ContainerAPIClient", package: "container")
            ]
        ),
        .testTarget(
            name: "GasCanAppleAttachTests",
            dependencies: ["GasCanAppleAttach"]
        )
    ]
)
