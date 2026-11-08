# Turns off Windows 11's three- and four-finger touch gestures for the signed-in user, now,
# so apps get every finger. Run it before the real input tests:
#
#   & .github/scripts/installer/touch-gestures-off.ps1
#
# While those gestures are on (Settings, Bluetooth & devices, Touch), Windows holds every
# touch contact back until it knows the contact is not one of its gestures, and keeps three
# or more fingers from every app. Writing TouchGestureSetting under HKCU\Control Panel\Desktop
# is not enough: win32k reads that value when the user signs in, and a runner cannot sign
# out. Settings changes it live through SystemParametersInfo, with an action that winuser.h
# does not name. win32k keeps its per-user settings in one table, in the order of their
# actions, and TouchGestureSetting sits where the action pair 0x2030 (get) and 0x2031 (set)
# falls. The set below is checked by reading the value back both from win32k and from the
# registry, so a wrong action fails here, with what it changed, rather than in the tests.
#
# Windows before build 22000 has no such gestures, and its actions in that range may mean
# something else, so it is left alone.

$ErrorActionPreference = 'Stop'

$build = [Environment]::OSVersion.Version.Build
$desktop = 'HKCU:\Control Panel\Desktop'
$stored = { (Get-ItemProperty -Path $desktop -Name TouchGestureSetting -ErrorAction SilentlyContinue).TouchGestureSetting }
if ($build -lt 22000) {
    Write-Host "touch gestures: Windows build $build has no three- and four-finger touch gestures"
    return
}

Add-Type -Namespace Dsa -Name Spi -MemberDefinition @'
[DllImport("user32.dll", SetLastError = true)]
public static extern bool SystemParametersInfoW(uint action, uint uiParam, ref uint value, uint flags);
[DllImport("user32.dll", SetLastError = true)]
public static extern bool SystemParametersInfoW(uint action, uint uiParam, IntPtr value, uint flags);
'@

$get = [uint32]0x2030
$set = [uint32]0x2031
# SPIF_UPDATEINIFILE | SPIF_SENDCHANGE: keep it in the user's profile and tell every window.
$saveAndTell = [uint32]3

function Read-Gestures {
    $value = [uint32]0
    if ([Dsa.Spi]::SystemParametersInfoW($get, 0, [ref]$value, 0)) { return $value }
    $null
}

# The neighbouring actions too, so a set that lands on another setting shows which.
function Read-Neighbours {
    (0x2026, 0x2028, 0x202A, 0x202C, 0x202E, 0x2030, 0x2032, 0x2034, 0x2036, 0x2038 | ForEach-Object {
        $value = [uint32]0
        $read = [Dsa.Spi]::SystemParametersInfoW([uint32]$_, 0, [ref]$value, 0)
        '0x{0:X4}={1}' -f $_, $(if ($read) { $value } else { '?' })
    }) -join ' '
}

$before = Read-Gestures
$neighbours = Read-Neighbours
Write-Host "touch gestures before: Windows build $build reads $before, the registry holds '$(& $stored)'; $neighbours"
if ($null -eq $before) {
    throw "Windows build $build would not say whether its three- and four-finger touch gestures are on (SystemParametersInfo 0x2030): $([ComponentModel.Win32Exception]::new([Runtime.InteropServices.Marshal]::GetLastWin32Error()).Message)"
}
# The newer actions take their value in pvParam itself (as SPI_SETCARETTIMEOUT does); if this
# one takes a pointer to it instead, the second form gives it that.
$turnedOff = [Dsa.Spi]::SystemParametersInfoW($set, 0, [IntPtr]::Zero, $saveAndTell)
$why = [Runtime.InteropServices.Marshal]::GetLastWin32Error()
if (-not $turnedOff -or (Read-Gestures) -ne 0) {
    $zero = [uint32]0
    $turnedOff = [Dsa.Spi]::SystemParametersInfoW($set, 0, [ref]$zero, $saveAndTell)
    if (-not $turnedOff) { $why = [Runtime.InteropServices.Marshal]::GetLastWin32Error() }
}
if (-not $turnedOff) {
    throw "Windows would not turn off its three- and four-finger touch gestures (SystemParametersInfo 0x2031): $([ComponentModel.Win32Exception]::new($why).Message)"
}
$after = Read-Gestures
$saved = & $stored
Write-Host "touch gestures after: Windows reads $after, the registry holds '$saved'; $(Read-Neighbours)"
if ($after -ne 0 -or $saved -ne 0) {
    throw "the three- and four-finger touch gestures are still on: Windows reads $after and the registry holds TouchGestureSetting '$saved' after SystemParametersInfo 0x2031; before it the actions read $neighbours, after it $(Read-Neighbours)"
}
