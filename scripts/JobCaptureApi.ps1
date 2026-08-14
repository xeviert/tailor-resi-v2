param(
  [int]$Port = 3000
)

$ErrorActionPreference = 'Stop'

Add-Type -AssemblyName System.Net

$Root = Split-Path -Parent (Split-Path -Parent $PSCommandPath)
$Captures = Join-Path $Root 'captures'
$LatestCapture = Join-Path $Captures 'latest.json'

function Ensure-CaptureDir {
  New-Item -ItemType Directory -Force -Path $Captures | Out-Null
}

function New-JsonResponse([int]$StatusCode, $Body) {
  $json = $Body | ConvertTo-Json -Depth 50
  return @{
    StatusCode = $StatusCode
    ContentType = 'application/json; charset=utf-8'
    Body = $json
  }
}

function New-TextResponse([int]$StatusCode, [string]$Body) {
  return @{
    StatusCode = $StatusCode
    ContentType = 'text/plain; charset=utf-8'
    Body = $Body
  }
}

function Read-HttpRequest($Stream) {
  $buffer = New-Object byte[] 8192
  $data = New-Object System.Collections.Generic.List[byte]
  $headerEnd = -1

  while ($headerEnd -lt 0) {
    $read = $Stream.Read($buffer, 0, $buffer.Length)
    if ($read -le 0) { throw 'Client closed connection before sending headers.' }
    for ($i = 0; $i -lt $read; $i++) { $data.Add($buffer[$i]) }

    for ($i = 3; $i -lt $data.Count; $i++) {
      if ($data[$i - 3] -eq 13 -and $data[$i - 2] -eq 10 -and $data[$i - 1] -eq 13 -and $data[$i] -eq 10) {
        $headerEnd = $i
        break
      }
    }
  }

  $headerBytes = $data.GetRange(0, $headerEnd + 1).ToArray()
  $headersRaw = [System.Text.Encoding]::ASCII.GetString($headerBytes)
  $headerLines = $headersRaw -split "`r`n" | Where-Object { $_.Length -gt 0 }
  if ($headerLines.Count -eq 0) { throw 'Missing request line.' }

  $requestLine = $headerLines[0] -split ' '
  if ($requestLine.Count -lt 2) { throw "Invalid request line: $($headerLines[0])" }

  $headers = @{}
  foreach ($line in $headerLines | Select-Object -Skip 1) {
    $idx = $line.IndexOf(':')
    if ($idx -gt 0) {
      $name = $line.Substring(0, $idx).Trim().ToLowerInvariant()
      $value = $line.Substring($idx + 1).Trim()
      $headers[$name] = $value
    }
  }

  $contentLength = 0
  if ($headers.ContainsKey('content-length')) {
    [void][int]::TryParse($headers['content-length'], [ref]$contentLength)
  }

  $bodyStart = $headerEnd + 1
  $availableBody = $data.Count - $bodyStart
  while ($availableBody -lt $contentLength) {
    $read = $Stream.Read($buffer, 0, [Math]::Min($buffer.Length, $contentLength - $availableBody))
    if ($read -le 0) { break }
    for ($i = 0; $i -lt $read; $i++) { $data.Add($buffer[$i]) }
    $availableBody = $data.Count - $bodyStart
  }

  $body = ''
  if ($contentLength -gt 0) {
    $bodyBytes = $data.GetRange($bodyStart, $contentLength).ToArray()
    $body = [System.Text.Encoding]::UTF8.GetString($bodyBytes)
  }

  $target = $requestLine[1]
  $path = if ($target -match '^https?://') {
    ([uri]$target).AbsolutePath
  } else {
    ($target -split '\?', 2)[0]
  }
  if ([string]::IsNullOrWhiteSpace($path)) { $path = '/' }

  return @{
    Method = $requestLine[0].ToUpperInvariant()
    Path = $path
    Headers = $headers
    Body = $body
  }
}

function Write-HttpResponse($Stream, $Response) {
  $bodyBytes = [System.Text.Encoding]::UTF8.GetBytes([string]$Response.Body)
  $reason = switch ($Response.StatusCode) {
    200 { 'OK' }
    201 { 'Created' }
    204 { 'No Content' }
    400 { 'Bad Request' }
    404 { 'Not Found' }
    405 { 'Method Not Allowed' }
    500 { 'Internal Server Error' }
    default { 'OK' }
  }

  $headers = @(
    "HTTP/1.1 $($Response.StatusCode) $reason",
    "Content-Type: $($Response.ContentType)",
    "Content-Length: $($bodyBytes.Length)",
    'Access-Control-Allow-Origin: *',
    'Access-Control-Allow-Methods: GET, POST, OPTIONS',
    'Access-Control-Allow-Headers: Content-Type',
    'Connection: close',
    '',
    ''
  ) -join "`r`n"

  $headerBytes = [System.Text.Encoding]::ASCII.GetBytes($headers)
  $Stream.Write($headerBytes, 0, $headerBytes.Length)
  if ($bodyBytes.Length -gt 0) {
    $Stream.Write($bodyBytes, 0, $bodyBytes.Length)
  }
}

function Save-Capture($Payload) {
  Ensure-CaptureDir

  $receivedAt = (Get-Date).ToUniversalTime().ToString('o')
  $record = [ordered]@{
    receivedAt = $receivedAt
    sourceUrl = $Payload.sourceUrl
    pageTitle = $Payload.pageTitle
    score = $Payload.score
    capturedCount = $Payload.capturedCount
    json = $Payload.json
  }

  $safeTimestamp = $receivedAt -replace '[:.]', ''
  $capturePath = Join-Path $Captures "$safeTimestamp-job.json"
  $json = $record | ConvertTo-Json -Depth 80
  Set-Content -LiteralPath $capturePath -Value $json -Encoding UTF8
  Set-Content -LiteralPath $LatestCapture -Value $json -Encoding UTF8

  return @{
    receivedAt = $receivedAt
    capturePath = $capturePath
  }
}

function Handle-Request($Request) {
  if ($Request.Method -eq 'OPTIONS') {
    return New-TextResponse 204 ''
  }

  if ($Request.Method -eq 'GET' -and $Request.Path -eq '/health') {
    return New-JsonResponse 200 @{
      app = 'resi-tailor'
      status = 'ok'
    }
  }

  if ($Request.Method -eq 'POST' -and $Request.Path -eq '/analyze') {
    if ([string]::IsNullOrWhiteSpace($Request.Body)) {
      return New-JsonResponse 400 @{ ok = $false; error = 'Request body is required.' }
    }

    try {
      $payload = $Request.Body | ConvertFrom-Json
    } catch {
      return New-JsonResponse 400 @{ ok = $false; error = 'Request body must be valid JSON.' }
    }

    $saved = Save-Capture $payload
    $relativeLatest = 'captures/latest.json'
    $relativeCapture = Resolve-Path -LiteralPath $saved.capturePath -Relative

    return New-JsonResponse 200 @{
      ok = $true
      saved = $relativeLatest
      capture = $relativeCapture
      receivedAt = $saved.receivedAt
    }
  }

  if ($Request.Path -eq '/health' -or $Request.Path -eq '/analyze') {
    return New-JsonResponse 405 @{ ok = $false; error = 'Method not allowed.' }
  }

  return New-JsonResponse 404 @{ ok = $false; error = 'Not found.' }
}

function Start-LoopbackListeners([int]$ListenPort) {
  $listeners = New-Object System.Collections.Generic.List[System.Net.Sockets.TcpListener]
  foreach ($address in @([System.Net.IPAddress]::Loopback, [System.Net.IPAddress]::IPv6Loopback)) {
    try {
      $listener = [System.Net.Sockets.TcpListener]::new($address, $ListenPort)
      $listener.Start()
      $listeners.Add($listener)
    } catch {
      Write-Host "[job-api] could not listen on $address`:$ListenPort ($($_.Exception.Message))"
    }
  }

  if ($listeners.Count -eq 0) {
    throw "Could not listen on localhost:$ListenPort"
  }

  return $listeners
}

Ensure-CaptureDir

$listeners = Start-LoopbackListeners $Port

Write-Host "[job-api] listening on http://localhost:$Port"
Write-Host '[job-api] press Ctrl+C to stop'

try {
  while ($true) {
    $client = $null
    foreach ($listener in $listeners) {
      if ($listener.Pending()) {
        $client = $listener.AcceptTcpClient()
        break
      }
    }

    if ($null -eq $client) {
      Start-Sleep -Milliseconds 25
      continue
    }

    try {
      $stream = $client.GetStream()
      try {
        $request = Read-HttpRequest $stream
        $response = Handle-Request $request
        Write-HttpResponse $stream $response
        Write-Host "[job-api] $($request.Method) $($request.Path) -> $($response.StatusCode)"
      } catch {
        $response = New-JsonResponse 500 @{ ok = $false; error = $_.Exception.Message }
        Write-HttpResponse $stream $response
        Write-Host "[job-api] error -> $($_.Exception.Message)"
      } finally {
        $stream.Dispose()
      }
    } finally {
      $client.Close()
    }
  }
} finally {
  foreach ($listener in $listeners) {
    $listener.Stop()
  }
}
