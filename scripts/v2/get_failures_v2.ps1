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
                $pass = $testCase.summaryResult.pass
                $results += [PSCustomObject]@{
                    Suite = $content.name
                    Test = $testCase.name
                    Pass = $pass
                }
            }
        }
    }
    return $results
}

$ws1Results = Get-FailedTests "workspace"
$ws2Results = Get-FailedTests "workspace2"

$allUniqueTests = ($ws1Results + $ws2Results) | Select-Object -Unique Suite, Test

$report = foreach ($t in $allUniqueTests) {
    $results1 = $ws1Results | Where-Object { $_.Suite -eq $t.Suite -and $_.Test -eq $t.Test }
    $results2 = $ws2Results | Where-Object { $_.Suite -eq $t.Suite -and $_.Test -eq $t.Test }
    
    $total1 = $results1.Count
    $fail1 = ($results1 | Where-Object { $_.Pass -eq $false }).Count
    if ($null -eq $fail1) { $fail1 = 0 }

    $total2 = $results2.Count
    $fail2 = ($results2 | Where-Object { $_.Pass -eq $false }).Count
    if ($null -eq $fail2) { $fail2 = 0 }

    if ($fail1 -gt 0 -or $fail2 -gt 0) {
        [PSCustomObject]@{
            Suite = $t.Suite
            Test = $t.Test
            WS1_Total = $total1
            WS1_Fails = $fail1
            WS2_Total = $total2
            WS2_Fails = $fail2
        }
    }
}

$report | ConvertTo-Json
