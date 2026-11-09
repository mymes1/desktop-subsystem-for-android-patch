// The ARM probe's one library, built for arm64-v8a only. NativeActivity calls
// ANativeActivity_onCreate through the native bridge, so this code runs translated. It does a
// little work in a loop, which the translator has to translate into its exec region, and then
// logs a line the boot check waits for.
#include <android/log.h>
#include <android/native_activity.h>
#include <stddef.h>
#include <unistd.h>

#define PROBE_TAG "DsaArmProbe"

__attribute__((visibility("default"))) void ANativeActivity_onCreate(ANativeActivity* activity,
                                                                     void* saved_state,
                                                                     size_t saved_state_size) {
    (void)activity;
    (void)saved_state;
    (void)saved_state_size;
    volatile unsigned long sum = 0;
    for (unsigned long i = 0; i < 100000; i++) {
        sum += i * i;
    }
    __android_log_print(ANDROID_LOG_INFO, PROBE_TAG, "DSA ARM probe ran as arm64 in process %d (%lu)",
                        (int)getpid(), (unsigned long)sum);
}
