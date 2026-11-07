# x86_64 only, as Cuttlefish's vsoc_x86_64_only board sets it, so the code is built for the
# same CPU as the image's own.
TARGET_CPU_ABI := x86_64
TARGET_ARCH := x86_64
TARGET_ARCH_VARIANT := silvermont

TARGET_NO_KERNEL := true
TARGET_NO_BOOTLOADER := true
