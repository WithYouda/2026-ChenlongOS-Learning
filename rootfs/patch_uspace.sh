#!/usr/bin/env bash
set -e

TARGET=$(find ~/.cargo/git/checkouts -type f \
  -path "*axcpu-*/src/riscv/uspace.rs")

if [ -z "$TARGET" ]; then
  echo "❌ uspace.rs not found"
  exit 1
fi

echo "✅ Found:"
echo "  $TARGET"

# 备份
cp "$TARGET" "$TARGET.bak"
echo "📦 Backup created: $TARGET.bak"

# 替换 Sstatus 初始化
if grep -q "Sstatus::from_bits(0)" "$TARGET"; then
  sed -i "s/Sstatus::from_bits(0)/Sstatus::from_bits(1 << 23)/" "$TARGET"
  echo "✅ Patched Sstatus init"
else
  echo "⚠️ Pattern not found, no changes made"
fi
