# Homebrew formula for a tap (github.com/Cloveian/homebrew-emoticond, as
# Formula/emoticond.rb): `brew install Cloveian/emoticond/emoticond`.
# The sha256 values are from the release's SHA256SUMS.
class Emoticond < Formula
  desc "Search kaomoji by how you feel"
  homepage "https://github.com/Cloveian/emoticond"
  version "1.0.0"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    on_arm do
      url "https://github.com/Cloveian/emoticond/releases/download/v#{version}/emoticond-v#{version}-aarch64-apple-darwin.tar.gz"
      sha256 "9666657d148cb9a2c28476ae656e0857ac47e0ca45676eac1f4817a979b8a877"
    end
  end

  on_linux do
    on_intel do
      url "https://github.com/Cloveian/emoticond/releases/download/v#{version}/emoticond-v#{version}-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "a28f34b06aaf0dd34cb9b78ab3ae8690f0477f3eb45608cae9ec94a69191cb00"
    end
    on_arm do
      url "https://github.com/Cloveian/emoticond/releases/download/v#{version}/emoticond-v#{version}-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "225836511f94cc9a821a1c08f7183f72713e71aa7073182daaeeb444e0281b84"
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
