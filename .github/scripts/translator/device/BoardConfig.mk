# x86_64 only, with arm64 as the native bridge guest, as the Digitalis emulator board
# (device/generic/goldfish/board/emu64xa) sets it.
TARGET_CPU_ABI := x86_64
TARGET_ARCH := x86_64
TARGET_ARCH_VARIANT := x86_64

TARGET_NATIVE_BRIDGE_ARCH := arm64
TARGET_NATIVE_BRIDGE_ARCH_VARIANT := armv8-a
TARGET_NATIVE_BRIDGE_CPU_VARIANT := generic
TARGET_NATIVE_BRIDGE_ABI := arm64-v8a

TARGET_NO_KERNEL := true
TARGET_NO_BOOTLOADER := true
