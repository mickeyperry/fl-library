# Registers the FL Library Explorer integration for .flp files. Run elevated. Undo with uninstall.ps1.
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $MyInvocation.MyCommand.Path
$clsid = '{7C0B4E7A-52F1-4B8E-9D7C-2F1A6B0C4D11}'
$built = Join-Path $root 'shellext\target\release\flprops.dll'
$bin = Join-Path $root 'bin'
$schema = Join-Path $bin 'flprops.propdesc'
# Python used for the scanner and menu actions: config.json "pythonw", else the first pythonw.exe on PATH
$cfgFile = Join-Path $root 'config.json'
$pythonw = if (Test-Path $cfgFile) { (Get-Content $cfgFile -Raw | ConvertFrom-Json).pythonw } else { $null }
if (-not $pythonw) { $pythonw = (Get-Command pythonw.exe -ErrorAction SilentlyContinue).Source }
if (-not $pythonw -or -not (Test-Path $pythonw)) { throw 'pythonw.exe not found: install Python 3.10+ or set "pythonw" in config.json' }

# Explorer keeps a loaded DLL locked, so each install gets a fresh file name
New-Item -ItemType Directory -Force $bin | Out-Null
if (-not [Environment]::Is64BitProcess) { throw 'run this from 64-bit PowerShell (Explorer is 64-bit)' }
$sha = [BitConverter]::ToString([Security.Cryptography.SHA1]::Create().ComputeHash([IO.File]::ReadAllBytes($built))) -replace '-'
$dll = Join-Path $bin ("flprops-{0}.dll" -f $sha.Substring(0, 8))
if (-not (Test-Path $dll)) { Copy-Item $built $dll }
# drop older builds; ones still loaded by Explorer are locked and simply stay until next time
Get-ChildItem $bin -Filter 'flprops-*.dll' | Where-Object { $_.FullName -ne $dll } |
    ForEach-Object { Remove-Item $_.FullName -Force -ErrorAction SilentlyContinue }
Copy-Item (Join-Path $root 'shellext\flprops.propdesc') $schema -Force

Add-Type -Namespace FL -Name Prop -MemberDefinition @'
[DllImport("propsys.dll", CharSet = CharSet.Unicode)] public static extern int PSRegisterPropertySchema(string path);
[DllImport("propsys.dll", CharSet = CharSet.Unicode)] public static extern int PSUnregisterPropertySchema(string path);
[DllImport("shell32.dll")] public static extern void SHChangeNotify(int eventId, int flags, IntPtr a, IntPtr b);
'@
[FL.Prop]::PSUnregisterPropertySchema($schema) | Out-Null
$hr = [FL.Prop]::PSRegisterPropertySchema($schema)
if ($hr -lt 0) { throw ('PSRegisterPropertySchema failed: 0x{0:X8}' -f $hr) }

function Set-Key($path, $values) {
    if (-not (Test-Path -LiteralPath $path)) { New-Item -Path $path -Force | Out-Null }  # -Force on an existing key wipes its values
    foreach ($k in $values.Keys) {
        $type = if ($values[$k] -is [int]) { 'DWord' } else { 'String' }
        Set-ItemProperty -LiteralPath $path -Name $k -Value $values[$k] -Type $type
    }
}

$config = Get-Content (Join-Path $root 'config.json') -Raw | ConvertFrom-Json
$flExe = Get-ChildItem 'C:\Program Files\Image-Line\FL Studio*\FL64.exe' -ErrorAction SilentlyContinue |
    Sort-Object LastWriteTime | Select-Object -Last 1
Set-Key 'HKLM:\SOFTWARE\FLLibrary' @{
    Db      = (Join-Path $root 'library.db')
    Ctl     = (Join-Path $root 'flctl.py')
    Pythonw = $pythonw
    FlExe   = $(if ($flExe) { $flExe.FullName } else { '' })
    Everything    = ([uri]$config.everything_url).Authority
    SearchExclude = (($config.exclude_prefixes | ForEach-Object { '!"' + $_ + '"' }) -join ' ')
}

# every installed FL Studio, by major version, so a project can be opened in the version it was made with
$installs = @{}
Get-ChildItem 'C:\Program Files\Image-Line\FL Studio*\FL*.exe', 'C:\Program Files (x86)\Image-Line\FL Studio*\FL*.exe' -ErrorAction SilentlyContinue |
    Where-Object { $_.Name -match '^FL(64)?\.exe$' } | ForEach-Object {
        $ver = [version]($_.VersionInfo.FileVersion -replace '[^\d.]')
        if ($ver.Major -lt 2 -and $_.Directory.Name -match 'FL Studio (\d+)') { $ver = [version]("$($Matches[1]).0.0.0") }  # FL 11/12 report 1.1.x
        $rank = '{0:D4}{1:D4}{2:D4}{3:D6}{4}' -f $ver.Major, $ver.Minor, [Math]::Max($ver.Build, 0), [Math]::Max($ver.Revision, 0), [int]($_.Name -eq 'FL64.exe')
        if (-not $installs[$ver.Major] -or $rank -gt $installs[$ver.Major].Rank) { $installs[$ver.Major] = @{ Rank = $rank; Exe = $_.FullName } }
    }
$flValues = @{ FlMajors = (($installs.Keys | Sort-Object) -join ',') }
foreach ($major in $installs.Keys) { $flValues["Fl_$major"] = $installs[$major].Exe }
Set-Key 'HKLM:\SOFTWARE\FLLibrary' $flValues

# companion panel: flpanel.exe sits over Explorer's preview-pane area (Alt+P) and follows the
# current folder / selection. It replaces the earlier preview-handler registration.
$panel = '{3E9A1D52-8C47-4B6F-A1E3-5D7F2B9C6A21}'
foreach ($key in @(
        "HKLM:\SOFTWARE\Classes\CLSID\$panel",
        'HKLM:\SOFTWARE\Classes\.flp\shellex\{8895b1c6-b41f-4c1c-a562-0d564250836f}',
        'HKLM:\SOFTWARE\Classes\Directory\shellex\{8895b1c6-b41f-4c1c-a562-0d564250836f}')) {
    if (Test-Path -LiteralPath $key) { Remove-Item -LiteralPath $key -Recurse -Force }
}
Remove-ItemProperty -LiteralPath 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\PreviewHandlers' -Name $panel -ErrorAction SilentlyContinue
$exe = Join-Path $bin 'flpanel.exe'
Get-Process flpanel -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Milliseconds 400
Copy-Item (Join-Path $root 'shellext\target\release\flpanel.exe') $exe -Force
foreach ($ico in 'fl_on.ico', 'fl_off.ico') { Copy-Item (Join-Path $root "icons\$ico") (Join-Path $bin $ico) -Force }
Set-ItemProperty -LiteralPath 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Run' -Name 'FLLibraryPanel' -Value ('"{0}"' -f $exe)
Start-Process explorer.exe -ArgumentList ('"{0}"' -f $exe)  # via Explorer so it runs un-elevated
Set-Key "HKLM:\SOFTWARE\Classes\CLSID\$clsid" @{ '(default)' = 'FL Library property handler' }  # isolated: runs in a surrogate, so a slow share or a bug can never hang or crash Explorer
Remove-ItemProperty -LiteralPath "HKLM:\SOFTWARE\Classes\CLSID\$clsid" -Name DisableProcessIsolation -ErrorAction SilentlyContinue
Set-Key "HKLM:\SOFTWARE\Classes\CLSID\$clsid\InprocServer32" @{ '(default)' = $dll; ThreadingModel = 'Both' }
Set-Key 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\PropertySystem\PropertyHandlers\.flp' @{ '(default)' = $clsid }

$assoc = 'HKLM:\SOFTWARE\Classes\SystemFileAssociations\.flp'
$all = 'FLLibrary.FLVersion;System.Music.BeatsPerMinute;FLLibrary.Status;System.Rating;System.Keywords;FLLibrary.HoursWorked;FLLibrary.MissingSamples;FLLibrary.Versions;FLLibrary.Channels;System.Title;System.Author;System.Music.Genre;System.Comment'
Set-Key $assoc @{
    FullDetails     = "prop:System.PropGroup.Description;$all;System.PropGroup.FileSystem;System.ItemNameDisplay;System.ItemFolderPathDisplay;System.Size;System.DateCreated;System.DateModified"
    PreviewDetails  = "prop:$all;System.DateModified;System.Size"
    InfoTip         = 'prop:FLLibrary.FLVersion;System.Music.BeatsPerMinute;FLLibrary.Status;System.Rating;System.Keywords;FLLibrary.HoursWorked;FLLibrary.MissingSamples;System.DateModified;System.Size'
    ExtendedTileInfo = 'prop:FLLibrary.FLVersion;System.Music.BeatsPerMinute;FLLibrary.Status'
}

# right-click > FL Library
$menu = "$assoc\shell\FLLibrary"
if (Test-Path $menu) { Remove-Item $menu -Recurse -Force }
Set-Key $menu @{ MUIVerb = 'FL Library'; SubCommands = '' }
$ctl = '"{0}" "{1}"' -f $pythonw, (Join-Path $root 'flctl.py')
$items = [ordered]@{
    '01samples'  = @('Show samples (missing / moved)', 'samples "%1"', 0)
    '02versions' = @('Find all versions in Everything', 'versions "%1"', 0)
    '10idea'   = @('Status: idea', 'status idea "%1"', 0x20)
    '11wip'    = @('Status: WIP', 'status wip "%1"', 0)
    '12mixing' = @('Status: mixing', 'status mixing "%1"', 0)
    '13done'   = @('Status: done', 'status done "%1"', 0)
    '14dead'   = @('Status: dead', 'status dead "%1"', 0)
    '15none'   = @('Status: clear', 'status none "%1"', 0)
    '20r5'     = @('Rating: 5 stars', 'rating 5 "%1"', 0x20)
    '21r4'     = @('Rating: 4 stars', 'rating 4 "%1"', 0)
    '22r3'     = @('Rating: 3 stars', 'rating 3 "%1"', 0)
    '23r2'     = @('Rating: 2 stars', 'rating 2 "%1"', 0)
    '24r1'     = @('Rating: 1 star', 'rating 1 "%1"', 0)
    '25r0'     = @('Rating: clear', 'rating 0 "%1"', 0)
    '30tags'   = @('Tags...', 'tags "%1"', 0x20)
    '31notes'  = @('Notes...', 'notes "%1"', 0)
    '40rescan' = @('Rescan library now', 'rescan', 0x20)
}
foreach ($k in $items.Keys) {
    $label, $cmdArgs, $flags = $items[$k]
    $values = @{ MUIVerb = $label }
    if ($flags) { $values.CommandFlags = $flags }
    Set-Key "$menu\shell\$k" $values
    Set-Key "$menu\shell\$k\command" @{ '(default)' = "$ctl $cmdArgs" }
}

# keep the index fresh: incremental scan every 30 minutes
$scan = '"{0}" "{1}"' -f $pythonw, (Join-Path $root 'scanner.py')
schtasks /Create /F /TN 'FL Library scan' /SC MINUTE /MO 30 /TR $scan | Out-Null

[FL.Prop]::SHChangeNotify(0x08000000, 0, [IntPtr]::Zero, [IntPtr]::Zero)  # SHCNE_ASSOCCHANGED
Write-Output "installed: $dll"
