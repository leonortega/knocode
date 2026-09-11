class Knocode < Formula
  desc "AI Runtime for coding agents — local daemon with UDS+MessagePack"
  homepage "https://github.com/leonortega/knocode"
  # TODO: switch to a prebuilt-binary bottle once Linux/macOS release artifacts exist
  # (the GitHub release workflow currently publishes Windows x64 only).
  version "0.9.11"
  # TODO: switch to a bottle (prebuilt binary) once Linux/macOS release artifacts exist
  url "https://github.com/leonortega/knocode/archive/v0.9.11.tar.gz"
  sha256 "REPLACE_WITH_SHA_AFTER_RELEASE"
  license "MIT"
  depends_on "rust" => :build

  def install
    system "cargo", "build", "--release"
    bin.install "target/release/knocode"
    bin.install "target/release/knocode-daemon"
  end

  service do
    run [opt_bin/"knocode-daemon"]
    keep_alive true
    log_path var/"log/knocode.log"
    error_log_path var/"log/knocode.error.log"
  end

  test do
    assert_match "0.9.11", shell_output("#{bin}/knocode --version")
    assert_match "All critical checks", shell_output("#{bin}/knocode doctor")
  end
end
