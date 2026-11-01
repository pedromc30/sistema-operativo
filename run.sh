#!/usr/bin/env bash
# Compila el kernel y lo arranca en QEMU por UEFI (OVMF) con Limine.
# El bootloader (BOOTX64.EFI) solo se ejecuta dentro del emulador.
set -euo pipefail
cd "$(dirname "$0")"

cargo build ${FEATURES:+--features "$FEATURES"}
rm -rf esp && mkdir -p esp/EFI/BOOT esp/boot
cp third_party_limine/BOOTX64.EFI esp/EFI/BOOT/BOOTX64.EFI
cp limine.conf esp/limine.conf
cp target/x86_64-unknown-none/debug/rustos esp/boot/kernel

# Copia de las variables UEFI para no tocar las del sistema.
cp --update=none /usr/share/OVMF/OVMF_VARS_4M.fd ovmf_vars.fd

# Salida serie por stdout, sin ventana. TIMEOUT (segundos) sirve para CI.
exec timeout "${TIMEOUT:-0}" qemu-system-x86_64 -machine q35 -m 256M \
  -drive if=pflash,format=raw,readonly=on,file=/usr/share/OVMF/OVMF_CODE_4M.fd \
  -drive if=pflash,format=raw,file=ovmf_vars.fd \
  -drive format=raw,file=fat:rw:esp \
  -serial stdio -display none -no-reboot
