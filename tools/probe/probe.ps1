# Battery discovery prototype: lists every accessory this machine can report a battery level for.
# Runs as a normal user. Usage: pwsh -File probe.ps1
$ErrorActionPreference = 'Stop'

Write-Host "`n== Bluetooth (Windows battery property) ==" -ForegroundColor Cyan
$btKey = '{104EA319-6EE2-4701-BD47-8DDBF425BBE5} 2'
$bt = Get-PnpDevice | Where-Object { $_.InstanceId -match '^(BTHENUM\\DEV_|BTHLE\\DEV_|BTHENUM\\\{|BTHLEDEVICE\\)' } |
    ForEach-Object {
        $p = Get-PnpDeviceProperty -InstanceId $_.InstanceId -KeyName $btKey -ErrorAction SilentlyContinue
        [pscustomobject]@{
            Name      = $_.FriendlyName
            Connected = $_.Status -eq 'OK'
            Battery   = if ($null -ne $p.Data) { "$($p.Data)%" } else { '' }
            Id        = $_.InstanceId
        }
    }
$btDevices = $bt | Where-Object { $_.Id -match '^(BTHENUM\\DEV_|BTHLE\\DEV_)' }
$btWithBattery = $bt | Where-Object Battery
# The battery value can sit on the device node or on one of its service nodes; match them by MAC address.
$(foreach ($d in $btDevices) {
    $mac = $d.Id -replace '^.*DEV_([0-9A-Fa-f]{12}).*$', '$1'
    $batt = ($btWithBattery | Where-Object { $_.Id -match $mac } | Select-Object -First 1).Battery
    [pscustomobject]@{ Name = $d.Name; Connected = $d.Connected; Battery = if ($batt) { $batt } else { 'not reported' } }
}) | Format-Table -AutoSize

Write-Host "== Logitech receivers and devices (HID++ 2.0) ==" -ForegroundColor Cyan
Add-Type -TypeDefinition (Get-Content -Raw (Join-Path $PSScriptRoot 'HidPP.cs'))
$sw = [Diagnostics.Stopwatch]::StartNew()
[HidPP]::Probe() | Select-Object Interface, @{n='Slot';e={ if ($_.Index -eq 255) { 'direct' } elseif ($_.Index) { $_.Index } else { '' } }}, Name, Kind, Battery,
    @{n='Source / notes';e={ (@(($_.Source -replace '^HID\+\+ ', ''), $_.Notes) | Where-Object { $_ }) -join '; ' }} |
    Format-Table -AutoSize -Wrap
Write-Host ("Logitech probe took {0:N1} s" -f $sw.Elapsed.TotalSeconds)

# Receivers whose HID++ interface is owned by Logitech's legacy Unifying/SetPoint driver are invisible to normal apps.
$legacy = Get-PnpDevice -PresentOnly | Where-Object { $_.FriendlyName -eq 'Logitech Cordless Device' -and $_.InstanceId -match 'QID_([0-9A-F]{4})' }
if ($legacy) {
    Write-Host "`n== Blocked by legacy Logitech driver ==" -ForegroundColor Yellow
    $legacy | ForEach-Object {
        $wpid = [regex]::Match($_.InstanceId, 'QID_([0-9A-F]{4})').Groups[1].Value
        $rx = [regex]::Match($_.InstanceId, 'PID_([0-9A-F]{4})').Groups[1].Value
        [pscustomobject]@{ Receiver = "PID $rx"; DeviceWPID = $wpid; Name = (Get-PnpDeviceProperty -InstanceId $_.InstanceId -KeyName 'DEVPKEY_Device_BusReportedDeviceDesc' -ErrorAction SilentlyContinue).Data; Driver = (Get-PnpDeviceProperty -InstanceId $_.InstanceId -KeyName 'DEVPKEY_Device_DriverProvider' -ErrorAction SilentlyContinue).Data }
    } | Format-Table -AutoSize
}
