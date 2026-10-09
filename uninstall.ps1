# Removes the FL Library Explorer integration. Run elevated. The library database is left alone.
$root = Split-Path -Parent $MyInvocation.MyCommand.Path
$clsid = '{7C0B4E7A-52F1-4B8E-9D7C-2F1A6B0C4D11}'
$schema = Join-Path $root 'bin\flprops.propdesc'

Add-Type -Namespace FL -Name Prop -MemberDefinition @'
[DllImport("propsys.dll", CharSet = CharSet.Unicode)] public static extern int PSUnregisterPropertySchema(string path);
[DllImport("shell32.dll")] public static extern void SHChangeNotify(int eventId, int flags, IntPtr a, IntPtr b);
'@
[FL.Prop]::PSUnregisterPropertySchema($schema) | Out-Null

$assoc = 'HKLM:\SOFTWARE\Classes\SystemFileAssociations\.flp'
foreach ($key in @(
        'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\PropertySystem\PropertyHandlers\.flp',
        "HKLM:\SOFTWARE\Classes\CLSID\$clsid",
        'HKLM:\SOFTWARE\Classes\CLSID\{3E9A1D52-8C47-4B6F-A1E3-5D7F2B9C6A21}',
        'HKLM:\SOFTWARE\Classes\.flp\shellex\{8895b1c6-b41f-4c1c-a562-0d564250836f}',
        'HKLM:\SOFTWARE\Classes\Directory\shellex\{8895b1c6-b41f-4c1c-a562-0d564250836f}',
        "$assoc\shell\FLLibrary",
        'HKLM:\SOFTWARE\FLLibrary')) {
    if (Test-Path $key) { Remove-Item $key -Recurse -Force }
}
foreach ($name in 'FullDetails', 'PreviewDetails', 'InfoTip', 'ExtendedTileInfo') {
    Remove-ItemProperty -LiteralPath $assoc -Name $name -ErrorAction SilentlyContinue
}
Remove-ItemProperty -LiteralPath 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\PreviewHandlers' -Name '{3E9A1D52-8C47-4B6F-A1E3-5D7F2B9C6A21}' -ErrorAction SilentlyContinue
Remove-ItemProperty -LiteralPath 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Run' -Name 'FLLibraryPanel' -ErrorAction SilentlyContinue
Get-Process flpanel -ErrorAction SilentlyContinue | Stop-Process -Force
schtasks /Delete /F /TN 'FL Library scan' 2>$null | Out-Null
[FL.Prop]::SHChangeNotify(0x08000000, 0, [IntPtr]::Zero, [IntPtr]::Zero)
Write-Output 'uninstalled (restart Explorer to unload the DLL)'
