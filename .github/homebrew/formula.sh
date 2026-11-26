#!/usr/bin/env bash
# Print the Homebrew formula for docbake.
# Usage: formula.sh <version> <SHA256SUMS file>
set -euo pipefail

version="$1"
sums="$2"

sha() {
  local s
  s="$(awk -v f="$1" '$2 == f { print $1 }' "$sums")"
  if [ -z "$s" ]; then
    echo "no checksum for $1 in $sums" >&2
    exit 1
  fi
  echo "$s"
}

base="https://github.com/growler/docbake/releases/download/v${version}"

cat <<EOF
class Docbake < Formula
  desc "Multi-format Markdown documentation compiler"
  homepage "https://github.com/growler/docbake"
  license "MIT"

  depends_on "typst" => :recommended
  depends_on "d2" => :optional
  depends_on "mermaid-cli" => :optional
  depends_on "plantuml" => :optional

  on_macos do
    on_arm do
      url "${base}/docbake-darwin-arm64.tar.gz"
      sha256 "$(sha docbake-darwin-arm64.tar.gz)"
    end
    on_intel do
      url "${base}/docbake-darwin-amd64.tar.gz"
      sha256 "$(sha docbake-darwin-amd64.tar.gz)"
    end
  end

  on_linux do
    on_intel do
      url "${base}/docbake-linux-amd64.tar.gz"
      sha256 "$(sha docbake-linux-amd64.tar.gz)"
    end
  end

  def install
    bin.install "docbake"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/docbake --version")
  end
end
EOF
