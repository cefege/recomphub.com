$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes

$installer = Get-ChildItem 'target/desktop-artifacts/release/*-setup.exe' | Select-Object -First 1
if (-not $installer) { throw 'Release NSIS installer is missing.' }
$installRoot = Join-Path $env:RUNNER_TEMP ('recomphub-native-' + [guid]::NewGuid())
$app = $null
$previousBrowserArguments = $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS
try {
    $setup = Start-Process $installer.FullName -ArgumentList @('/S', "/D=$installRoot") -Wait -PassThru
    if ($setup.ExitCode -ne 0) { throw "Installer failed with exit code $($setup.ExitCode)." }
    $executable = Join-Path $installRoot 'recomphub.exe'
    if (-not (Test-Path $executable)) { throw 'Installer did not install the desktop executable.' }

    $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--force-renderer-accessibility'
    $app = Start-Process $executable -PassThru
    $deadline = [DateTime]::UtcNow.AddSeconds(90)
    $ready = $false
    $scope = [System.Windows.Automation.TreeScope]::Descendants
    $nameProperty = [System.Windows.Automation.AutomationElement]::NameProperty
    $ignoreCase = [System.Windows.Automation.PropertyConditionFlags]::IgnoreCase
    while ([DateTime]::UtcNow -lt $deadline) {
        $app.Refresh()
        if ($app.HasExited) { throw "Desktop exited before rendering: $($app.ExitCode)." }
        if ($app.MainWindowHandle -ne [IntPtr]::Zero) {
            try {
                $window = [System.Windows.Automation.AutomationElement]::FromHandle($app.MainWindowHandle)
                $library = $window.FindFirst($scope, [System.Windows.Automation.PropertyCondition]::new($nameProperty, 'LIBRARY', $ignoreCase))
                $gameSearch = $window.FindFirst($scope, [System.Windows.Automation.PropertyCondition]::new($nameProperty, 'Search your games', $ignoreCase))
                $emptyLibrary = $window.FindFirst($scope, [System.Windows.Automation.PropertyCondition]::new($nameProperty, 'Your game library', $ignoreCase))
                if ($library -and $gameSearch -and $emptyLibrary) {
                    $ready = $true
                    break
                }
            } catch [System.Windows.Automation.ElementNotAvailableException] {
                # The WebView accessibility provider can change while its first document mounts.
            }
        }
        Start-Sleep -Milliseconds 250
    }
    if (-not $ready) { throw 'Installed desktop did not expose the signed-out game library and search controls.' }
    Write-Output 'Windows release installed and rendered the signed-out native library, game search and first-launch game panel.'
} finally {
    $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = $previousBrowserArguments
    if ($app) {
        $app.Refresh()
        if (-not $app.HasExited) {
            $null = $app.CloseMainWindow()
            if (-not $app.WaitForExit(5000)) { Stop-Process -Id $app.Id -Force }
        }
    }
    if (Test-Path $installRoot) { Remove-Item $installRoot -Recurse -Force }
}
