#!/usr/bin/env bash
# 将 linux 二进制打包为 Debian 软件包（供 bash-agent 聚合 apt 源使用）。
# 用法：scripts/build-deb.sh <版本> <二进制> [输出目录]
# 例：scripts/build-deb.sh v0.1.1 mcpc-linux-amd64
#
# 二进制名约定：<pkg>-linux-<arch>，arch 取 amd64/arm64；
# 输出 <pkg>_<version>_<arch>.deb。

set -euo pipefail

VERSION_INPUT="${1:?用法：$0 <版本> <二进制> [输出目录]}"
BIN="${2:?缺少二进制文件名，如 mcpc-linux-amd64}"
OUTPUT_DIR="${3:-.}"

PACKAGE="mcpc"
MAINTAINER="Lloyd Zhou <lloydzhou@qq.com>"
HOMEPAGE="https://github.com/lloydzhou/mcpc"
VERSION="${VERSION_INPUT#v}"
ARCH="${BIN##*-}"

command -v dpkg-deb >/dev/null 2>&1 || {
  echo "错误：需要 dpkg-deb。" >&2
  exit 1
}

if command -v dpkg >/dev/null 2>&1; then
  dpkg --validate-version "$VERSION" >/dev/null 2>&1 || {
    echo "错误：不是合法的 Debian 版本：$VERSION" >&2
    exit 1
  }
fi

[[ "$ARCH" == amd64 || "$ARCH" == arm64 ]] || {
  echo "错误：无法从 $BIN 推断架构（期望 amd64/arm64）" >&2
  exit 1
}
[[ "$BIN" == "$PACKAGE-linux-"* ]] || {
  echo "错误：二进制名应为 $PACKAGE-linux-<arch>，实际 $BIN" >&2
  exit 1
}
[[ -f "$BIN" ]] || {
  echo "错误：二进制不存在：$BIN" >&2
  exit 1
}

mkdir -p "$OUTPUT_DIR"
OUTPUT_DIR="$(cd "$OUTPUT_DIR" && pwd)"

# 可复现构建：固定时间戳
if [[ -z "${SOURCE_DATE_EPOCH:-}" ]]; then
  if command -v git >/dev/null 2>&1 && git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    SOURCE_DATE_EPOCH="$(git log -1 --format=%ct)"
  else
    SOURCE_DATE_EPOCH=0
  fi
fi
if (( SOURCE_DATE_EPOCH < 315532800 )); then
  SOURCE_DATE_EPOCH=315532800
fi
export SOURCE_DATE_EPOCH

WORK_DIR="$(mktemp -d "${TMPDIR:-/tmp}/mcpc-deb.XXXXXX")"
trap 'rm -rf "$WORK_DIR"' EXIT INT TERM

PKG_DIR="$WORK_DIR/$ARCH"
BIN_DIR="$PKG_DIR/usr/bin"
mkdir -p "$PKG_DIR/DEBIAN" "$BIN_DIR"
chmod 0755 "$PKG_DIR/DEBIAN"
install -m 0755 "$BIN" "$BIN_DIR/$PACKAGE"

cat > "$PKG_DIR/DEBIAN/control" <<EOF
Package: $PACKAGE
Version: $VERSION
Section: utils
Priority: optional
Architecture: $ARCH
Maintainer: $MAINTAINER
Depends: ca-certificates
Homepage: $HOMEPAGE
Description: MCP client CLI with daemon-held sessions
 Talks to MCP servers over Streamable HTTP or stdio JSON-RPC and keeps
 a small local daemon so that multiple CLI invocations share the same
 connection/session.
EOF
chmod 0644 "$PKG_DIR/DEBIAN/control"

(
  cd "$PKG_DIR"
  find usr -type f -print0 | LC_ALL=C sort -z | xargs -0 md5sum
) > "$PKG_DIR/DEBIAN/md5sums"
chmod 0644 "$PKG_DIR/DEBIAN/md5sums"

find "$PKG_DIR" -print0 | xargs -0 touch -h -d "@$SOURCE_DATE_EPOCH"

OUTPUT="$OUTPUT_DIR/${PACKAGE}_${VERSION}_${ARCH}.deb"
rm -f "$OUTPUT"
dpkg-deb --root-owner-group -Zxz -z9 --build "$PKG_DIR" "$OUTPUT"
echo "已生成：$OUTPUT"
