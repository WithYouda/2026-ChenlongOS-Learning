#!/usr/bin/env bash
set -e

TARGET=$(find ~/.cargo/registry/src -type f \
  -path "*riscv-*/src/register/sstatus.rs")

if [ -z "$TARGET" ]; then
  echo "❌ sstatus.rs not found"
  exit 1
fi

echo "✅ Found:"
echo "  $TARGET"

# 备份
cp "$TARGET" "$TARGET.bak"
echo "📦 Backup created: $TARGET.bak"

# 替换 MASK 常量
if grep -q "0x8000_0003_000d_e122" "$TARGET"; then
  sed -i "s/0x8000_0003_000d_e122/0x8000_0003_018D_E122/" "$TARGET"
  echo "✅ Patched MASK value"
else
  echo "⚠️ Pattern not found, no changes made"
fi
