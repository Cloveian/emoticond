# Homebrew formula for a tap (github.com/Cloveian/homebrew-emoticond, as
# Formula/emoticond.rb): `brew install Cloveian/emoticond/emoticond`.
# The sha256 values are from the release's SHA256SUMS.
class Emoticond < Formula
  desc "(Legitimately) clever kaomoji search engine"
  homepage "https://github.com/Cloveian/emoticond"
  version "1.0.1"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    on_arm do
      url "https://github.com/Cloveian/emoticond/releases/download/v#{version}/emoticond-v#{version}-aarch64-apple-darwin.tar.gz"
      sha256 "77ac04b2524422c7e0cf9b9a3fb4f7a2ad69bfefa2ce5097ec04f146affab667"
    end
  end

  on_linux do
    on_intel do
      url "https://github.com/Cloveian/emoticond/releases/download/v#{version}/emoticond-v#{version}-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "89a8c72daa44a439ae23b1e827b207f51b976628948cffa2d14e00c37e17c0e3"
    end
    on_arm do
      url "https://github.com/Cloveian/emoticond/releases/download/v#{version}/emoticond-v#{version}-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "5159ceb0de9a3c92be46b3bac12c6e08dac0bfdf98d8403d137e6c25b4c5bb98"
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
