$ErrorActionPreference = 'Continue'
$base = 'http://localhost:8080'
$exe = 'd:\WinAPPDev\MiniMask\target\debug\MiniMask.exe'

# 1. login
$r = Invoke-RestMethod -Uri "$base/api/auth/login" -Method Post -ContentType 'application/json' -Body '{"username":"admin","password":"admin"}' -SessionVariable sess
Write-Host "[1] login ok: $($r.username)"

# 2. create a client (NOT connected yet)
$c = Invoke-RestMethod -Uri "$base/api/clients" -Method Post -ContentType 'application/json' -Body '{"name":"hot-client"}' -WebSession $sess
$id = $c.client.id
$tok = $c.token
Write-Host "[2] created client id=$id"

# 3. start local test server on 9000
$server = Start-Process -FilePath 'node' -ArgumentList 'd:\WinAPPDev\MiniMask\test_server.js' -RedirectStandardOutput 'd:\WinAPPDev\MiniMask\test_server.log' -RedirectStandardError 'd:\WinAPPDev\MiniMask\test_server.err' -PassThru -WindowStyle Hidden
Start-Sleep -Seconds 1
Write-Host "[3] local test server started pid=$($server.Id)"

# 4. connect the companion client (no mappings exist yet)
$client = Start-Process -FilePath $exe -ArgumentList @('client','--server','127.0.0.1:7443','--tls','--id',$id,'--token',$tok,'--server-name','localhost') -RedirectStandardOutput 'd:\WinAPPDev\MiniMask\client.log' -RedirectStandardError 'd:\WinAPPDev\MiniMask\client.err' -WorkingDirectory 'd:\WinAPPDev\MiniMask' -PassThru -WindowStyle Hidden
Write-Host "[4] client connected pid=$($client.Id)"
Start-Sleep -Seconds 2

# 5. verify session with zero mappings
$s = Invoke-RestMethod -Uri "$base/api/sessions" -WebSession $sess
Write-Host "[5] sessions (expect 1 online, 0 mappings): $($s | ConvertTo-Json -Compress)"

# 6. HOT-RELOAD: create a mapping WHILE the client is connected
$body = '{"client_id":"' + $id + '","name":"web","remote_port":18081,"local_addr":"127.0.0.1:9000"}'
$m = Invoke-RestMethod -Uri "$base/api/mappings" -Method Post -ContentType 'application/json' -Body $body -WebSession $sess
Write-Host "[6] created mapping :18081 -> 127.0.0.1:9000 (hot-reload, id=$($m.id))"
Start-Sleep -Seconds 1

# 7. curl the public port -> should reach the tunneled service immediately
$resp = curl.exe -s http://localhost:18081/
Write-Host "[7] public :18081 response: $resp"

# 8. stats
$stats = Invoke-RestMethod -Uri "$base/api/stats" -WebSession $sess
Write-Host "[8] stats: total_conns=$($stats.total_conns) active=$($stats.active_conns)"

# cleanup
Stop-Process -Id $client.Id -Force -ErrorAction SilentlyContinue
Stop-Process -Id $server.Id -Force -ErrorAction SilentlyContinue
