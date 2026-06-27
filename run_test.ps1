$ErrorActionPreference = 'Continue'
$base = 'http://localhost:8080'
$exe = 'd:\WinAPPDev\MiniMask\target\debug\MiniMask.exe'

# 1. login
$r = Invoke-RestMethod -Uri "$base/api/auth/login" -Method Post -ContentType 'application/json' -Body '{"username":"admin","password":"admin"}' -SessionVariable sess
Write-Host "[1] login ok: $($r.username)"

# 2. create client
$c = Invoke-RestMethod -Uri "$base/api/clients" -Method Post -ContentType 'application/json' -Body '{"name":"test-client"}' -WebSession $sess
$id = $c.client.id
$tok = $c.token
Write-Host "[2] created client id=$id"

# 3. create mapping (before the client connects)
$body = '{"client_id":"' + $id + '","name":"web","remote_port":18080,"local_addr":"127.0.0.1:9000"}'
$m = Invoke-RestMethod -Uri "$base/api/mappings" -Method Post -ContentType 'application/json' -Body $body -WebSession $sess
Write-Host "[3] created mapping :18080 -> 127.0.0.1:9000 (id=$($m.id))"

# 4. start node test server
$server = Start-Process -FilePath 'node' -ArgumentList 'd:\WinAPPDev\MiniMask\test_server.js' -RedirectStandardOutput 'd:\WinAPPDev\MiniMask\test_server.log' -RedirectStandardError 'd:\WinAPPDev\MiniMask\test_server.err' -PassThru -WindowStyle Hidden
Start-Sleep -Seconds 1
Write-Host "[4] local test server started pid=$($server.Id)"

# 5. start companion client (TLS)
$client = Start-Process -FilePath $exe -ArgumentList @('client','--server','127.0.0.1:7443','--tls','--id',$id,'--token',$tok,'--server-name','localhost') -RedirectStandardOutput 'd:\WinAPPDev\MiniMask\client.log' -RedirectStandardError 'd:\WinAPPDev\MiniMask\client.err' -WorkingDirectory 'd:\WinAPPDev\MiniMask' -PassThru -WindowStyle Hidden
Write-Host "[5] companion client started pid=$($client.Id)"
Start-Sleep -Seconds 3

# 6. check sessions
$sessions = Invoke-RestMethod -Uri "$base/api/sessions" -WebSession $sess
Write-Host "[6] sessions: $($sessions | ConvertTo-Json -Compress)"

# 7. curl the public port -> should reach the tunneled service
$resp = curl.exe -s http://localhost:18080/
Write-Host "[7] public :18080 response: $resp"

# 8. stats
$stats = Invoke-RestMethod -Uri "$base/api/stats" -WebSession $sess
Write-Host "[8] stats: total_conns=$($stats.total_conns) active=$($stats.active_conns) bytes_in=$($stats.bytes_in) bytes_out=$($stats.bytes_out)"

# cleanup
Stop-Process -Id $client.Id -Force -ErrorAction SilentlyContinue
Stop-Process -Id $server.Id -Force -ErrorAction SilentlyContinue
