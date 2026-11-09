# Desktop Subsystem for Android (DSA)
# Compiles Google Play services and the Play Store again without an app image. dsa-gapps.rc runs
# it as the shell, with dalvik.vm.appimageformat cleared, which is what keeps ART from making an
# app image, and puts the format back once it ends.
#
# A package is compiled when the code of its base APK comes from a profile-guided compile
# (speed-profile) this script did not make: the Play Store's install or update, or ART's
# background job. Those are the compiles that make an app image. ART records the script's own
# compile with the reason "cmdline", and such a package is left alone, so a boot or a screen
# going off costs one status read per package and nothing more. The compile keeps the
# package's profile, so the same code is compiled ahead of time as before.
#
# The agent asks again (vendor.dsa.gapps.compile) when either package changes. A request that
# comes while the script runs is seen at its end, and the packages are looked at again.

packages="com.google.android.gms com.android.vending"
tag=DsaGappsCompile

# The compiler filter and the compile reason of the base APK's code for the primary ABI, as
# "speed-profile install-dm", from ART's dump of the package: its first "path:" line is the
# base APK, and the first line after it that names the primary ABI holds its status. Empty
# when the package is not installed.
status() {
    cmd package art dump "$1" 2>/dev/null |
        sed -n '/path: /,/\[primary-abi\]/{
/\[primary-abi\]/{
s/.*\[status=\([^]]*\)\] \[reason=\([^]]*\)\].*/\1 \2/p
q
}
}'
}

compile_all() {
    for package in $packages; do
        current=$(status "$package")
        case "$current" in
            "" | *" cmdline") continue ;;
            speed-profile\ * | everything-profile\ *) ;;
            *) continue ;;
        esac
        log -t "$tag" "compiling $package again without an app image (it was $current)"
        if cmd package compile -m speed-profile -p PRIORITY_BACKGROUND -f "$package" > /dev/null 2>&1; then
            log -t "$tag" "compiled $package without an app image: $(status "$package")"
        else
            log -p w -t "$tag" "could not compile $package again; it keeps its app image"
        fi
    done
}

while :; do
    asked=$(getprop vendor.dsa.gapps.compile)
    compile_all
    [ "$(getprop vendor.dsa.gapps.compile)" = "$asked" ] && break
done
log -t "$tag" "checked $packages"
