class ClaudeCount < Formula
  desc "macOS menu bar app for monitoring Claude usage"
  homepage "https://github.com/deepak-agarwal/claude-count"
  license "MIT"
  head "https://github.com/deepak-agarwal/claude-count.git", branch: "main"

  depends_on :macos
  depends_on "rust" => :build

  def install
    system "cargo", "build", "--release", "--locked"
    bin.install "target/release/claude-code-usage-monitor" => "claude-count"
  end

  def caveats
    <<~EOS
      Claude Count is a macOS menu bar app.

      Start it with:
        claude-count
    EOS
  end

  test do
    assert_predicate bin/"claude-count", :exist?
  end
end
