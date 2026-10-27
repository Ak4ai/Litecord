$n = [Environment]::ProcessorCount
1..$n | ForEach-Object { Start-Job { $x = 0.0; while ($true) { $x += [math]::Sqrt([double](Get-Random)) } } } | Out-Null
Start-Sleep 70
Get-Job | Stop-Job; Get-Job | Remove-Job
