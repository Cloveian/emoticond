# Homebrew formula for a tap (github.com/Cloveian/homebrew-emoticond, as
# Formula/emoticond.rb): `brew install Cloveian/emoticond/emoticond`.
# The sha256 values are from the release's SHA256SUMS.
class Emoticond < Formula
  desc "(Legitimately) clever kaomoji search engine"
  homepage "https://github.com/Cloveian/emoticond"
  version "1.0.2"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    on_arm do
      url "https://github.com/Cloveian/emoticond/releases/download/v#{version}/emoticond-v#{version}-aarch64-apple-darwin.tar.gz"
      sha256 "23ec2662d74fbebe1b24f15c2b962b40f11ea2a3e072a903ec8789935521fc0e"
    end
  end

  on_linux do
    on_intel do
      url "https://github.com/Cloveian/emoticond/releases/download/v#{version}/emoticond-v#{version}-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "0c36e08dace5c6201d606cc2cd1b88db10a08c10890c3cb03fff0046c85520ad"
    end
    on_arm do
      url "https://github.com/Cloveian/emoticond/releases/download/v#{version}/emoticond-v#{version}-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "30b8c93fa3f66addc0f60a4b0695d401ed8f6e0878fff6a5bcd20df387611d8d"
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
