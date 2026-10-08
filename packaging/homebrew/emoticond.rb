# Homebrew formula for a tap (github.com/Cloveian/homebrew-emoticond, as
# Formula/emoticond.rb): `brew install Cloveian/emoticond/emoticond`.
# Fill in the sha256 values from the release's SHA256SUMS.
class Emoticond < Formula
  desc "Search kaomoji by how you feel"
  homepage "https://github.com/Cloveian/emoticond"
  version "1.0.1"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    on_arm do
      url "https://github.com/Cloveian/emoticond/releases/download/v#{version}/emoticond-v#{version}-aarch64-apple-darwin.tar.gz"
      sha256 "FILL_IN"
    end
  end

  on_linux do
    on_intel do
      url "https://github.com/Cloveian/emoticond/releases/download/v#{version}/emoticond-v#{version}-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "FILL_IN"
    end
    on_arm do
      url "https://github.com/Cloveian/emoticond/releases/download/v#{version}/emoticond-v#{version}-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "FILL_IN"
    end
  end

  def install
    bin.install "emoticond"
    doc.install "README.md"
  end

  def caveats
    <<~EOS
      emoticond needs its data file (~13 MB). Download it with:
        emoticond data fetch
    EOS
  end

  test do
    assert_match "emoticond", shell_output("#{bin}/emoticond --version")
  end
end
