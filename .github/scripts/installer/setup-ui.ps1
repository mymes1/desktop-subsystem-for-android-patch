# Helpers for the installer test: find setup.exe's window and its controls through UI
# Automation, press buttons, and save pictures of the window. Dot-source it:
#
#   . .github/scripts/installer/setup-ui.ps1

Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes, System.Windows.Forms, System.Drawing

$script:Auto = [System.Windows.Automation.AutomationElement]
$script:Scope = [System.Windows.Automation.TreeScope]

# The automation id of the window the functions below work on: setup.exe's, unless a test
# sets another, such as the APK install window's (ApkInstallWindow).
$script:WindowId = "SetupWindow"

function Find-SetupWindow {
    $condition = New-Object System.Windows.Automation.PropertyCondition($Auto::AutomationIdProperty, $script:WindowId)
    $Auto::RootElement.FindFirst($Scope::Children, $condition)
}

# Waits until the setup window shows the control with this automation id (and, with
# -Enabled, until it can be pressed). Returns the control.
function Wait-SetupControl([string]$Id, [int]$Seconds = 60, [switch]$Enabled) {
    $condition = New-Object System.Windows.Automation.PropertyCondition($Auto::AutomationIdProperty, $Id)
    $deadline = (Get-Date).AddSeconds($Seconds)
    while ((Get-Date) -lt $deadline) {
        $window = Find-SetupWindow
        if ($window) {
            $control = $window.FindFirst($Scope::Descendants, $condition)
            if ($control -and -not $control.Current.IsOffscreen -and (-not $Enabled -or $control.Current.IsEnabled)) {
                return $control
            }
        }
        Start-Sleep -Milliseconds 250
    }
    Show-Desktop "timeout-$Id"
    throw "setup.exe did not show $Id within $Seconds seconds"
}

# Says what is on the screen: the top-level windows and a picture of the whole screen.
function Show-Desktop([string]$Name) {
    $Auto::RootElement.FindAll($Scope::Children, [System.Windows.Automation.Condition]::TrueCondition) | ForEach-Object {
        $process = Get-Process -Id $_.Current.ProcessId -ErrorAction SilentlyContinue
        Write-Host ("window '{0}' id '{1}' process {2}" -f $_.Current.Name, $_.Current.AutomationId, $process.ProcessName)
    }
    Get-Process | Where-Object { $_.ProcessName -match 'setup|DsaSetup' } | ForEach-Object { Write-Host "process $($_.ProcessName) $($_.Id)" }
    New-Item -ItemType Directory -Force screens | Out-Null
    $screen = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
    $bitmap = New-Object System.Drawing.Bitmap $screen.Width, $screen.Height
    $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
    $graphics.CopyFromScreen($screen.Location, [System.Drawing.Point]::Empty, $screen.Size)
    $bitmap.Save("$PWD\screens\$Name.png")
    $graphics.Dispose()
    $bitmap.Dispose()
}

function Invoke-SetupButton([string]$Id, [int]$Seconds = 60) {
    $button = Wait-SetupControl $Id -Seconds $Seconds -Enabled
    $button.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern).Invoke()
}

function Get-SetupText([string]$Id) {
    (Wait-SetupControl $Id).Current.Name
}

# Saves a picture of the setup window and 32 px around it (the shadow and rounded corners).
function Save-SetupScreen([string]$Path) {
    $window = Find-SetupWindow
    if (-not $window) { throw "no setup window to take a picture of" }
    Save-WindowScreen $window $Path
}

# The top-level window of the process with this id, waiting for it to appear.
function Wait-ProcessWindow([int]$ProcessId, [int]$Seconds = 60) {
    $condition = New-Object System.Windows.Automation.PropertyCondition($Auto::ProcessIdProperty, $ProcessId)
    $deadline = (Get-Date).AddSeconds($Seconds)
    while ((Get-Date) -lt $deadline) {
        $window = $Auto::RootElement.FindFirst($Scope::Children, $condition)
        if ($window) { return $window }
        Start-Sleep -Milliseconds 500
    }
    $null
}

# Selects the item named $Name inside $Window (such as a navigation item). Returns whether it
# could.
function Select-Named($Window, [string]$Name) {
    $condition = New-Object System.Windows.Automation.PropertyCondition($Auto::NameProperty, $Name)
    $item = $Window.FindFirst($Scope::Descendants, $condition)
    if (-not $item) { return $false }
    $pattern = $null
    if ($item.TryGetCurrentPattern([System.Windows.Automation.SelectionItemPattern]::Pattern, [ref]$pattern)) {
        $pattern.Select()
        return $true
    }
    if ($item.TryGetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern, [ref]$pattern)) {
        $pattern.Invoke()
        return $true
    }
    $false
}

# Saves a picture of $Window and 32 px around it.
function Save-WindowScreen($Window, [string]$Path) {
    Start-Sleep -Milliseconds 600   # let the window finish drawing
    $r = $Window.Current.BoundingRectangle
    $screen = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
    $x = [Math]::Max(0, [int]$r.X - 32)
    $y = [Math]::Max(0, [int]$r.Y - 32)
    $w = [Math]::Min($screen.Width - $x, [int]$r.Width + 64)
    $h = [Math]::Min($screen.Height - $y, [int]$r.Height + 64)
    $bitmap = New-Object System.Drawing.Bitmap $w, $h
    $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
    $graphics.CopyFromScreen($x, $y, 0, 0, (New-Object System.Drawing.Size $w, $h))
    $bitmap.Save($Path)
    $graphics.Dispose()
    $bitmap.Dispose()
    Write-Host "saved $Path"
}

# The text elements of $Window named exactly $Label.
function Find-RowTexts($Window, [string]$Label) {
    $condition = New-Object System.Windows.Automation.AndCondition(
        (New-Object System.Windows.Automation.PropertyCondition($Auto::NameProperty, $Label)),
        (New-Object System.Windows.Automation.PropertyCondition($Auto::ControlTypeProperty, [System.Windows.Automation.ControlType]::Text)))
    @($Window.FindAll($Scope::Descendants, $condition))
}

# Whether a row's name has room: a row squeezed to no width wraps its name at every letter, so
# the text is either empty or far taller than one line.
function Test-RowText($Text) {
    $r = $Text.Current.BoundingRectangle
    -not $r.IsEmpty -and $r.Width -ge 20 -and $r.Height -gt 0 -and $r.Height -le 60
}

# Whether $Element lies inside $Window. UI Automation's IsOffscreen stays false for a row
# scrolled out of a page's scroll viewer, so it cannot say this.
function Test-InWindow($Window, $Element) {
    $w = $Window.Current.BoundingRectangle
    $r = $Element.Current.BoundingRectangle
    -not $Element.Current.IsOffscreen -and $r.Top -ge $w.Top -and $r.Bottom -le $w.Bottom
}

# Fails unless every label is a laid out row of the list on the page in $Window, and at least
# $MinVisible of them are in view. A count above the list says nothing about this: the blank
# Apps page had the right count.
function Assert-RowsShown($Window, [string[]]$Labels, [int]$MinVisible = 1, [int]$Seconds = 30) {
    $deadline = (Get-Date).AddSeconds($Seconds)
    do {
        $missing = @($Labels | Where-Object { -not (Find-RowTexts $Window $_ | Where-Object { Test-RowText $_ }) })
        $visible = @($Labels | Where-Object { Find-RowTexts $Window $_ | Where-Object { (Test-RowText $_) -and (Test-InWindow $Window $_) } })
        if ($missing.Count -eq 0 -and $visible.Count -ge $MinVisible) {
            Write-Host "rows laid out: $($Labels.Count), in view: $($visible.Count) ($($visible -join ', '))"
            return
        }
        Start-Sleep -Milliseconds 500
    } while ((Get-Date) -lt $deadline)
    $all = $Window.FindAll($Scope::Descendants, (New-Object System.Windows.Automation.PropertyCondition($Auto::ControlTypeProperty, [System.Windows.Automation.ControlType]::Text)))
    Write-Host "text on the page: $(($all | ForEach-Object { $_.Current.Name }) -join ' | ')"
    foreach ($label in $Labels) {
        foreach ($text in (Find-RowTexts $Window $label)) {
            Write-Host "'$label': $($text.Current.BoundingRectangle) offscreen=$($text.Current.IsOffscreen)"
        }
    }
    throw "the list shows no usable row for: $($missing -join ', '); $($visible.Count) rows in view, wanted $MinVisible"
}

# Scrolls the page's list to its end.
function Set-PageScrolledToEnd($Window) {
    $condition = New-Object System.Windows.Automation.PropertyCondition($Auto::IsScrollPatternAvailableProperty, $true)
    foreach ($candidate in $Window.FindAll($Scope::Descendants, $condition)) {
        $scroll = $candidate.GetCurrentPattern([System.Windows.Automation.ScrollPattern]::Pattern)
        if ($scroll.Current.VerticallyScrollable -and $candidate.Current.BoundingRectangle.X -gt ($Window.Current.BoundingRectangle.X + 200)) {
            $scroll.SetScrollPercent([System.Windows.Automation.ScrollPattern]::NoScroll, 100)
            Start-Sleep -Milliseconds 500
            return
        }
    }
    throw "the page has nothing that scrolls"
}

# The control of the row named $Label with this control type and, if given, this name.
function Find-RowControl($Window, [string]$Label, $ControlType, [string]$Name) {
    $condition = New-Object System.Windows.Automation.AndCondition(
        (New-Object System.Windows.Automation.PropertyCondition($Auto::HelpTextProperty, $Label)),
        (New-Object System.Windows.Automation.PropertyCondition($Auto::ControlTypeProperty, $ControlType)))
    $found = @($Window.FindAll($Scope::Descendants, $condition) | Where-Object { -not $Name -or $_.Current.Name -eq $Name })
    if ($found.Count -eq 0) { throw "the row '$Label' has no $($ControlType.ProgrammaticName) '$Name'" }
    $found[0]
}

# Apps use the light (1) or the dark (0) theme. setup.exe reads it when it starts.
function Set-AppTheme([switch]$Dark) {
    $key = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"
    if (-not (Test-Path $key)) { New-Item -Path $key -Force | Out-Null }
    Set-ItemProperty -Path $key -Name AppsUseLightTheme -Value ([int](-not $Dark)) -Type DWord
}

function Wait-SetupExit($Process, [int]$Seconds = 120) {
    if (-not $Process.WaitForExit($Seconds * 1000)) { throw "setup.exe did not finish within $Seconds seconds" }
    $Process.ExitCode
}

# Some runner images open Windows' first sign-in privacy screen over the whole desktop, which
# takes the mouse wheel and the foreground from the window under test. It is closed and kept
# from coming back.
function Close-PrivacyExperience {
    $key = 'HKCU:\Software\Policies\Microsoft\Windows\OOBE'
    New-Item -Path $key -Force | Out-Null
    Set-ItemProperty -Path $key -Name DisablePrivacyExperience -Value 1 -Type DWord
    Get-Process UserOOBEBroker, WWAHost -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
}
