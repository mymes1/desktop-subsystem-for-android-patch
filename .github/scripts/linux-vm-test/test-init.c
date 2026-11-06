// The init of the VM the launcher test boots with the runner's own kernel: it loads the
// modules the initrd carries, in the order of their numbered names (the virtio balloon
// driver and what it needs, when the kernel does not build them in), and then idles, so the
// guest's balloon driver is up while the test resizes, pauses and stops the VM.
#define _GNU_SOURCE
#include <fcntl.h>
#include <stdio.h>
#include <sys/syscall.h>
#include <unistd.h>

int main(void) {
    for (int i = 0; i < 100; i++) {
        char path[32];
        snprintf(path, sizeof path, "/modules/%02d.ko", i);
        int fd = open(path, O_RDONLY | O_CLOEXEC);
        if (fd < 0) {
            continue;
        }
        if (syscall(SYS_finit_module, fd, "", 0) != 0) {
            perror(path);
        }
        close(fd);
    }
    printf("dsa test init: modules loaded\n");
    fflush(stdout);
    for (;;) {
        pause();
    }
}
