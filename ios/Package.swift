// swift-tools-version: 6.0

import PackageDescription

let package = Package(
  name: "Viz",
  platforms: [
    .iOS(.v17),
    .macOS(.v14),
  ],
  products: [
    .library(
      name: "Viz",
      targets: ["Viz"]
    )
  ],
  targets: [
    .target(
      name: "Viz"
    )
  ]
)
