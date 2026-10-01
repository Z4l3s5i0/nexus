function Get-FailedTests($workspace) {
    $files = Get-ChildItem "$workspace\logs\*.json"
    $results = @()
    foreach ($file in $files) {
        $jsonStr = Get-Content $file.FullName -Raw
        if ([string]::IsNullOrWhiteSpace($jsonStr)) { continue }
        try {
            $content = $jsonStr | ConvertFrom-Json
        } catch {
            continue
        }
        if ($content -and $content.testCases) {
            foreach ($testId in $content.testCases.PSObject.Properties.Name) {
                $testCase = $content.testCases.$testId
                if ($testCase.summaryResult.pass -eq $false) {
                    $results += [PSCustomObject]@{
                        Suite = $content.name
                        Test = $testCase.name
                    }
                }
            }
        }
    }
    return $results
}

$ws1Failures = Get-FailedTests "workspace"
$ws2Failures = Get-FailedTests "workspace2"

$allUniqueFailures = ($ws1Failures + $ws2Failures) | Select-Object -Unique Suite, Test

$report = foreach ($fail in $allUniqueFailures) {
    $count1 = ($ws1Failures | Where-Object { $_.Suite -eq $fail.Suite -and $_.Test -eq $fail.Test }).Count
    if ($null -eq $count1) { $count1 = 0 }
    
    $count2 = ($ws2Failures | Where-Object { $_.Suite -eq $fail.Suite -and $_.Test -eq $fail.Test }).Count
    if ($null -eq $count2) { $count2 = 0 }

    [PSCustomObject]@{
        Suite = $fail.Suite
        Test = $fail.Test
        FailuresInWorkspace1 = $count1
        FailuresInWorkspace2 = $count2
    }
}

$report | ConvertTo-Json
