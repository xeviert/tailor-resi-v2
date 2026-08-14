param(
  [Parameter(Mandatory=$true)][ValidateSet('init','render','validate','fit','archive','pdf')] [string]$Command,
  [ValidateSet('en','fr')] [string]$Lang,
  [string]$Content,
  [string]$Out,
  [string]$Docx,
  [string]$Variant,
  [string]$Company,
  [string]$Role
)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.IO.Compression.FileSystem

$Root = Split-Path -Parent (Split-Path -Parent $PSCommandPath)
$Source = Join-Path $Root 'source'
$Templates = Join-Path $Root 'templates'
$ContentDir = Join-Path $Root 'content'
$Generated = Join-Path $Root 'generated'
$Variants = Join-Path $Root 'variants'
$Qa = Join-Path $Root 'qa'
$WNs = 'http://schemas.openxmlformats.org/wordprocessingml/2006/main'

$Config = @{
  en = @{
    source = 'Xevier_T_CV_en.docx'
    template = 'Xevier_T_CV_en.template.docx'
    generated = 'Xevier_T_CV_en.generated.docx'
    editable = @{
      3='experience.0.company_line'; 4='experience.0.title_line'; 5='experience.0.bullets.0'; 6='experience.0.bullets.1'; 7='experience.0.bullets.2'; 8='experience.0.bullets.3'
      9='experience.1.company_line'; 10='experience.1.title_line'; 11='experience.1.bullets.0'; 12='experience.1.bullets.1'
      13='experience.2.company_line'; 14='experience.2.title_line'; 15='experience.2.bullets.0'; 16='experience.2.bullets.1'
      17='experience.3.company_line'; 18='experience.3.title_line'; 19='experience.3.bullets.0'
      20='experience.4.company_line'; 21='experience.4.title_line'; 22='experience.4.bullets.0'
      23='experience.5.company_line'; 24='experience.5.title_line'; 25='experience.5.bullets.0'
      31='skills.frontend'; 32='skills.architecture_backend'; 33='skills.ai_data'; 34='skills.testing'; 35='skills.devops'; 36='skills.tools'
    }
    locked = @{ header=@(0,1); section_headings=@(2,26,30); education=@(27,28,29) }
  }
  fr = @{
    source = 'Xevier_T_CV_fr.docx'
    template = 'Xevier_T_CV_fr.template.docx'
    generated = 'Xevier_T_CV_fr.generated.docx'
    editable = @{
      3='experience.0.company_line'; 4='experience.0.title_line'; 5='experience.0.bullets.0'; 6='experience.0.bullets.1'; 7='experience.0.bullets.2'
      8='experience.1.company_line'; 9='experience.1.title_line'; 10='experience.1.bullets.0'; 11='experience.1.bullets.1'
      12='experience.2.company_line'; 13='experience.2.title_line'; 14='experience.2.bullets.0'; 15='experience.2.bullets.1'
      16='experience.3.company_line'; 17='experience.3.title_line'; 18='experience.3.bullets.0'
      19='experience.4.company_line'; 20='experience.4.title_line'; 21='experience.4.bullets.0'
      22='experience.5.company_line'; 23='experience.5.title_line'; 24='experience.5.bullets.0'
      30='skills.frontend'; 31='skills.architecture_backend'; 32='skills.ai_data'; 33='skills.testing'; 34='skills.devops'; 35='skills.tools'
    }
    locked = @{ header=@(0,1); section_headings=@(2,25,29); education=@(26,27,28) }
  }
}

function Ensure-Dirs {
  @($Source,$Templates,$ContentDir,$Generated,$Variants,$Qa) | ForEach-Object {
    New-Item -ItemType Directory -Force -Path $_ | Out-Null
  }
}

function Expand-Docx([string]$DocxPath, [string]$Dir) {
  if (Test-Path $Dir) { Remove-Item -LiteralPath $Dir -Recurse -Force }
  New-Item -ItemType Directory -Force -Path $Dir | Out-Null
  [System.IO.Compression.ZipFile]::ExtractToDirectory((Resolve-Path $DocxPath), $Dir)
}

function Compress-Docx([string]$Dir, [string]$DocxPath) {
  if (Test-Path $DocxPath) { Remove-Item -LiteralPath $DocxPath -Force }
  $fs = [System.IO.File]::Open($DocxPath, [System.IO.FileMode]::CreateNew)
  try {
    $zip = New-Object System.IO.Compression.ZipArchive($fs, [System.IO.Compression.ZipArchiveMode]::Create)
    try {
      Get-ChildItem -LiteralPath $Dir -Recurse -File | Sort-Object FullName | ForEach-Object {
        $entryName = $_.FullName.Substring($Dir.Length).TrimStart('\','/') -replace '\\','/'
        $entry = $zip.CreateEntry($entryName, [System.IO.Compression.CompressionLevel]::Optimal)
        $entryStream = $entry.Open()
        $fileStream = [System.IO.File]::OpenRead($_.FullName)
        try { $fileStream.CopyTo($entryStream) }
        finally {
          $fileStream.Dispose()
          $entryStream.Dispose()
        }
      }
    } finally {
      $zip.Dispose()
    }
  } finally {
    $fs.Dispose()
  }
}

function Load-Xml([string]$Path) {
  $xml = New-Object System.Xml.XmlDocument
  $xml.PreserveWhitespace = $true
  $xml.Load($Path)
  return $xml
}

function New-NsManager($Xml) {
  $ns = New-Object System.Xml.XmlNamespaceManager($Xml.NameTable)
  $ns.AddNamespace('w', $WNs)
  return $ns
}

function Get-Text($Node, $Ns) {
  $parts = @()
  $Node.GetElementsByTagName('t', $WNs) | ForEach-Object { $parts += $_.InnerText }
  return ($parts -join '')
}

function Get-NonEmptyParagraphs($Xml, $Ns) {
  $body = $Xml.GetElementsByTagName('body', $WNs)[0]
  $items = New-Object System.Collections.ArrayList
  foreach ($node in $body.GetElementsByTagName('p', $WNs)) {
    if ((Get-Text $node $Ns).Trim().Length -gt 0) { [void]$items.Add($node) }
  }
  return $items
}

function New-WElement($Xml, [string]$Name) {
  return $Xml.CreateElement('w', $Name, $WNs)
}

function Set-WAttr($Xml, $El, [string]$Name, [string]$Value) {
  $attr = $Xml.CreateAttribute('w', $Name, $WNs)
  $attr.Value = $Value
  [void]$El.Attributes.Append($attr)
}

function Get-WChild($Node, [string]$Name) {
  foreach ($child in $Node.ChildNodes) {
    if ($child.LocalName -eq $Name -and $child.NamespaceURI -eq $WNs) { return $child }
  }
  return $null
}

function Ensure-WChild($Xml, $Node, [string]$Name) {
  $child = Get-WChild $Node $Name
  if ($null -eq $child) {
    $child = New-WElement $Xml $Name
    [void]$Node.AppendChild($child)
  }
  return $child
}

function Remove-WChildren($Node, [string]$Name) {
  foreach ($child in @($Node.ChildNodes)) {
    if ($child.LocalName -eq $Name -and $child.NamespaceURI -eq $WNs) {
      [void]$Node.RemoveChild($child)
    }
  }
}

function Ensure-RunProperty($Xml, $RunProperties, [string]$Name) {
  if ($null -eq (Get-WChild $RunProperties $Name)) {
    [void]$RunProperties.AppendChild((New-WElement $Xml $Name))
  }
}

function Remove-RunProperty($RunProperties, [string]$Name) {
  if ($null -ne $RunProperties) { Remove-WChildren $RunProperties $Name }
}

function Set-ParagraphRightTab($Xml, $Paragraph) {
  $pPr = Ensure-WChild $Xml $Paragraph 'pPr'
  $tabs = Ensure-WChild $Xml $pPr 'tabs'
  foreach ($tab in @($tabs.GetElementsByTagName('tab', $WNs))) {
    [void]$tabs.RemoveChild($tab)
  }
  $tabEl = New-WElement $Xml 'tab'
  Set-WAttr $Xml $tabEl 'val' 'right'
  Set-WAttr $Xml $tabEl 'pos' '10800'
  [void]$tabs.AppendChild($tabEl)
}

function Wrap-Paragraph($Xml, $Paragraph, [string]$Tag) {
  $sdt = New-WElement $Xml 'sdt'
  $sdtPr = New-WElement $Xml 'sdtPr'
  $tagEl = New-WElement $Xml 'tag'
  Set-WAttr $Xml $tagEl 'val' $Tag
  $aliasEl = New-WElement $Xml 'alias'
  Set-WAttr $Xml $aliasEl 'val' $Tag
  $textEl = New-WElement $Xml 'text'
  [void]$sdtPr.AppendChild($tagEl)
  [void]$sdtPr.AppendChild($aliasEl)
  [void]$sdtPr.AppendChild($textEl)
  $content = New-WElement $Xml 'sdtContent'
  [void]$sdt.AppendChild($sdtPr)
  [void]$sdt.AppendChild($content)
  $parent = $Paragraph.ParentNode
  [void]$parent.ReplaceChild($sdt, $Paragraph)
  [void]$content.AppendChild($Paragraph)
}

function Set-Protection([string]$SettingsPath) {
  $xml = Load-Xml $SettingsPath
  $ns = New-NsManager $xml
  foreach ($old in @($xml.GetElementsByTagName('documentProtection', $WNs))) {
    [void]$old.ParentNode.RemoveChild($old)
  }
  $protection = New-WElement $xml 'documentProtection'
  Set-WAttr $xml $protection 'edit' 'forms'
  Set-WAttr $xml $protection 'enforcement' '1'
  Set-WAttr $xml $protection 'formatting' '0'
  [void]$xml.DocumentElement.InsertBefore($protection, $xml.DocumentElement.FirstChild)
  $xml.Save($SettingsPath)
}

function Get-LockedSnapshot([string]$LangCode, [string]$DocxPath) {
  $tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("resume_snapshot_" + [guid]::NewGuid())
  Expand-Docx $DocxPath $tmp
  try {
    $xml = Load-Xml (Join-Path $tmp 'word\document.xml')
    $ns = New-NsManager $xml
    $paragraphs = Get-NonEmptyParagraphs $xml $ns
    $out = [ordered]@{}
    foreach ($group in $Config[$LangCode].locked.Keys) {
      $values = @()
      foreach ($idx in $Config[$LangCode].locked[$group]) { $values += (Get-Text $paragraphs[$idx] $ns) }
      $out[$group] = $values
    }
    return $out
  } finally {
    if (Test-Path $tmp) { Remove-Item -LiteralPath $tmp -Recurse -Force }
  }
}

function Initialize-Templates([string]$OnlyLang) {
  Ensure-Dirs
  $langs = if ($OnlyLang) { @($OnlyLang) } else { @('en','fr') }
  foreach ($langCode in $langs) {
    $cfg = $Config[$langCode]
    $sourceDocx = Join-Path $Source $cfg.source
    if (!(Test-Path $sourceDocx)) {
      $rootDocx = Join-Path (Split-Path -Parent $Root) $cfg.source
      if (Test-Path $rootDocx) { Copy-Item -LiteralPath $rootDocx -Destination $sourceDocx -Force }
      else { throw "Missing source document: $sourceDocx" }
    }
    $tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("resume_template_" + [guid]::NewGuid())
    Expand-Docx $sourceDocx $tmp
    try {
      $docXml = Join-Path $tmp 'word\document.xml'
      $xml = Load-Xml $docXml
      $ns = New-NsManager $xml
      $paragraphs = Get-NonEmptyParagraphs $xml $ns
      foreach ($idx in ($cfg.editable.Keys | Sort-Object -Descending)) {
        Wrap-Paragraph $xml $paragraphs[[int]$idx] $cfg.editable[$idx]
      }
      if ($langCode -eq 'fr') { Apply-FrenchTemplateFixes $xml $ns }
      $xml.Save($docXml)
      Set-Protection (Join-Path $tmp 'word\settings.xml')
      Compress-Docx $tmp (Join-Path $Templates $cfg.template)
      Get-LockedSnapshot $langCode $sourceDocx | ConvertTo-Json -Depth 8 | Set-Content -Encoding UTF8 (Join-Path $ContentDir "locked.$langCode.json")
      Write-Host "[init] wrote $langCode template"
    } finally {
      if (Test-Path $tmp) { Remove-Item -LiteralPath $tmp -Recurse -Force }
    }
  }
}

function Get-PropertyValue($Obj, [string]$Name) {
  return $Obj.PSObject.Properties[$Name].Value
}

function Flatten-Content($Data) {
  $body = if ($Data.PSObject.Properties['content']) { $Data.content } else { $Data }
  $values = @{}
  for ($i = 0; $i -lt $body.experience.Count; $i++) {
    $job = $body.experience[$i]
    $companyLine = if ($job.location) { "$($job.company)`t$($job.location)" } else { "$($job.company)" }
    $titleLine = if ($job.dates) { "$($job.title)`t$($job.dates)" } else { "$($job.title)" }
    $values["experience.$i.company_line"] = $companyLine
    $values["experience.$i.title_line"] = $titleLine
    for ($j = 0; $j -lt $job.bullets.Count; $j++) {
      $values["experience.$i.bullets.$j"] = [string]$job.bullets[$j]
    }
  }
  foreach ($skill in $body.skills.PSObject.Properties) {
    $values["skills.$($skill.Name)"] = [string]$skill.Value
  }
  return $values
}

function New-TextRun($Xml, [string]$Text, $RunProperties) {
  $run = New-WElement $Xml 'r'
  if ($null -ne $RunProperties) { [void]$run.AppendChild($RunProperties.CloneNode($true)) }
  $t = New-WElement $Xml 't'
  $t.InnerText = $Text
  if ($Text.StartsWith(' ') -or $Text.EndsWith(' ')) {
    $space = $Xml.CreateAttribute('xml','space','http://www.w3.org/XML/1998/namespace')
    $space.Value = 'preserve'
    [void]$t.Attributes.Append($space)
  }
  [void]$run.AppendChild($t)
  return $run
}

function Replace-ParagraphText($Xml, $Paragraph, [string]$Value, $Ns, [string]$Tag = '') {
  $pPr = $null
  foreach ($child in $Paragraph.ChildNodes) {
    if ($child.LocalName -eq 'pPr' -and $child.NamespaceURI -eq $WNs) { $pPr = $child; break }
  }
  $firstRPr = $Paragraph.GetElementsByTagName('rPr', $WNs) | Select-Object -First 1
  foreach ($child in @($Paragraph.ChildNodes)) {
    if ($null -eq $pPr -or ![object]::ReferenceEquals($child, $pPr)) {
      [void]$Paragraph.RemoveChild($child)
    }
  }

  if ($Tag -match '^experience\.\d+\.(company_line|title_line)$') {
    Set-ParagraphRightTab $Xml $Paragraph
  }

  $parts = $Value -split "`t", 0, 'SimpleMatch'

  if ($Tag -match '^experience\.\d+\.title_line$' -and $parts.Count -gt 1) {
    $titleRPr = if ($null -ne $firstRPr) { $firstRPr.CloneNode($true) } else { New-WElement $Xml 'rPr' }
    Ensure-RunProperty $Xml $titleRPr 'b'
    Ensure-RunProperty $Xml $titleRPr 'i'

    $dateRPr = if ($null -ne $firstRPr) { $firstRPr.CloneNode($true) } else { New-WElement $Xml 'rPr' }
    Remove-RunProperty $dateRPr 'b'
    Ensure-RunProperty $Xml $dateRPr 'i'

    [void]$Paragraph.AppendChild((New-TextRun $Xml $parts[0] $titleRPr))
    [void]$Paragraph.AppendChild((New-WElement $Xml 'r')).AppendChild((New-WElement $Xml 'tab'))
    [void]$Paragraph.AppendChild((New-TextRun $Xml $parts[1] $dateRPr))
    return
  }

  if ($Tag -match '^skills\.') {
    $separator = $Value.IndexOf(':')
    if ($separator -ge 0) {
      $label = $Value.Substring(0, $separator + 1)
      $terms = $Value.Substring($separator + 1)

      $labelRPr = if ($null -ne $firstRPr) { $firstRPr.CloneNode($true) } else { New-WElement $Xml 'rPr' }
      Ensure-RunProperty $Xml $labelRPr 'b'

      $termsRPr = if ($null -ne $firstRPr) { $firstRPr.CloneNode($true) } else { New-WElement $Xml 'rPr' }
      Remove-RunProperty $termsRPr 'b'

      [void]$Paragraph.AppendChild((New-TextRun $Xml $label $labelRPr))
      [void]$Paragraph.AppendChild((New-TextRun $Xml $terms $termsRPr))
      return
    }
  }

  $run = New-WElement $Xml 'r'
  if ($null -ne $firstRPr) { [void]$run.AppendChild($firstRPr.CloneNode($true)) }
  for ($i = 0; $i -lt $parts.Count; $i++) {
    if ($i -gt 0) {
      [void]$run.AppendChild((New-WElement $Xml 'tab'))
    }
    $textRun = New-TextRun $Xml $parts[$i] $null
    foreach ($child in @($textRun.ChildNodes)) { [void]$run.AppendChild($child) }
  }
  [void]$Paragraph.AppendChild($run)
}

function Copy-ParagraphProperties($Xml, $FromParagraph, $ToParagraph) {
  $fromPPr = $null
  foreach ($child in $FromParagraph.ChildNodes) {
    if ($child.LocalName -eq 'pPr' -and $child.NamespaceURI -eq $WNs) { $fromPPr = $child; break }
  }
  if ($null -eq $fromPPr) { return }

  $toPPr = $null
  foreach ($child in $ToParagraph.ChildNodes) {
    if ($child.LocalName -eq 'pPr' -and $child.NamespaceURI -eq $WNs) { $toPPr = $child; break }
  }

  $newPPr = $fromPPr.CloneNode($true)
  if ($null -ne $toPPr) {
    [void]$ToParagraph.ReplaceChild($newPPr, $toPPr)
  } else {
    [void]$ToParagraph.InsertBefore($newPPr, $ToParagraph.FirstChild)
  }
}

function Get-SdtByTag($Xml, [string]$TagValue) {
  foreach ($sdt in $Xml.GetElementsByTagName('sdt', $WNs)) {
    foreach ($candidate in $sdt.GetElementsByTagName('tag', $WNs)) {
      if ($candidate.GetAttribute('val', $WNs) -eq $TagValue) { return $sdt }
      break
    }
  }
  return $null
}

function Ensure-RunItalic($Xml, $Run) {
  $rPr = $null
  foreach ($child in $Run.ChildNodes) {
    if ($child.LocalName -eq 'rPr' -and $child.NamespaceURI -eq $WNs) { $rPr = $child; break }
  }
  if ($null -eq $rPr) {
    $rPr = New-WElement $Xml 'rPr'
    [void]$Run.InsertBefore($rPr, $Run.FirstChild)
  }
  if ($rPr.GetElementsByTagName('i', $WNs).Count -eq 0) {
    [void]$rPr.AppendChild((New-WElement $Xml 'i'))
  }
}

function Apply-FrenchTemplateFixes($Xml, $Ns) {
  $referenceCompany = Get-SdtByTag $Xml 'experience.0.company_line'
  $werqCompany = Get-SdtByTag $Xml 'experience.1.company_line'
  if ($null -ne $werqCompany) {
    $p = $werqCompany.GetElementsByTagName('p', $WNs)[0]
    if ($null -ne $referenceCompany) {
      $referenceP = $referenceCompany.GetElementsByTagName('p', $WNs)[0]
      Copy-ParagraphProperties $Xml $referenceP $p
    }
    $eAcute = [char]0x00E9
    Replace-ParagraphText $Xml $p "Werq AI`tT$($eAcute)l$($eAcute)travail" $Ns
  }

  $firstTitle = Get-SdtByTag $Xml 'experience.0.title_line'
  if ($null -ne $firstTitle) {
    foreach ($run in $firstTitle.GetElementsByTagName('r', $WNs)) {
      Ensure-RunItalic $Xml $run
    }
  }
}

function Render-Resume([string]$LangCode, [string]$ContentPath, [string]$OutPath) {
  Ensure-Dirs
  $cfg = $Config[$LangCode]
  if (!$ContentPath) { $ContentPath = Join-Path $ContentDir "base.$LangCode.json" }
  if (!$OutPath) { $OutPath = Join-Path $Generated $cfg.generated }
  $template = Join-Path $Templates $cfg.template
  $data = Get-Content -Raw -Encoding UTF8 $ContentPath | ConvertFrom-Json
  $values = Flatten-Content $data
  $tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("resume_render_" + [guid]::NewGuid())
  Expand-Docx $template $tmp
  try {
    $docXml = Join-Path $tmp 'word\document.xml'
    $xml = Load-Xml $docXml
    $ns = New-NsManager $xml
    $found = @{}
    foreach ($sdt in $xml.GetElementsByTagName('sdt', $WNs)) {
      $tagNode = $null
      foreach ($candidate in $sdt.GetElementsByTagName('tag', $WNs)) { $tagNode = $candidate; break }
      if ($null -eq $tagNode) { continue }
      $tag = $tagNode.GetAttribute('val', $WNs)
      if (!$values.ContainsKey($tag)) { continue }
      $p = $null
      foreach ($candidate in $sdt.GetElementsByTagName('p', $WNs)) { $p = $candidate; break }
      if ($null -ne $p) {
        Replace-ParagraphText $xml $p $values[$tag] $ns $tag
        $found[$tag] = $true
      }
    }
    $missing = @($values.Keys | Where-Object { !$found.ContainsKey($_) } | Sort-Object)
    if ($missing.Count -gt 0) { throw "Missing template fields: $($missing -join ', ')" }
    $xml.Save($docXml)
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $OutPath) | Out-Null
    Compress-Docx $tmp $OutPath
    Write-Host "[render] wrote $OutPath"
  } finally {
    if (Test-Path $tmp) { Remove-Item -LiteralPath $tmp -Recurse -Force }
  }
}

function Validate-Resume([string]$LangCode, [string]$DocxPath) {
  $expected = Get-Content -Raw -Encoding UTF8 (Join-Path $ContentDir "locked.$LangCode.json") | ConvertFrom-Json
  $actual = Get-LockedSnapshot $LangCode $DocxPath
  $expectedJson = $expected | ConvertTo-Json -Depth 8 -Compress
  $actualJson = $actual | ConvertTo-Json -Depth 8 -Compress
  if ($expectedJson -ne $actualJson) { throw "Locked header, section heading, or education text changed." }
  Write-Host "[validate] locked sections unchanged for $LangCode"
}

function Archive-Variant([string]$LangCode, [string]$VariantPath, [string]$CompanyName, [string]$RoleName) {
  $slugRaw = "$(Get-Date -Format yyyy-MM-dd)-$CompanyName-$RoleName-$LangCode".ToLowerInvariant()
  $slug = ($slugRaw -replace '[^a-z0-9]+','-').Trim('-')
  $dest = Join-Path $Variants $slug
  New-Item -ItemType Directory -Force -Path $dest | Out-Null
  Copy-Item -LiteralPath $VariantPath -Destination (Join-Path $dest 'variant.json') -Force
  Render-Resume $LangCode (Join-Path $dest 'variant.json') (Join-Path $dest "Xevier_T_CV_$LangCode.$slug.docx")
  Write-Host "[archive] wrote $dest"
}

function Export-Pdf([string]$DocxPath, [string]$OutDir) {
  $knownPath = 'C:\Program Files\LibreOffice\program\soffice.com'
  $soffice = if (Test-Path $knownPath) { Get-Item $knownPath } else { Get-Command soffice -ErrorAction SilentlyContinue }
  if ($null -eq $soffice) { $soffice = Get-Command libreoffice -ErrorAction SilentlyContinue }
  if ($null -eq $soffice) {
    throw 'LibreOffice is required for PDF export and one-page validation. Install LibreOffice or add soffice to PATH.'
  }
  $sofficePath = if ($soffice.PSObject.Properties['Source'] -and $soffice.Source) { $soffice.Source } else { $soffice.FullName }
  New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
  $profile = Join-Path ([System.IO.Path]::GetTempPath()) ("resume_lo_profile_" + [guid]::NewGuid())
  New-Item -ItemType Directory -Force -Path $profile | Out-Null
  try {
    $profileUri = 'file:///' + ($profile -replace '\\','/')
    $process = Start-Process -FilePath $sofficePath -ArgumentList @(
      "-env:UserInstallation=$profileUri", '--headless', '--convert-to', 'pdf', '--outdir', $OutDir, $DocxPath
    ) -PassThru -NoNewWindow
    if (!$process.WaitForExit(30000)) {
      & taskkill.exe /PID $process.Id /T /F | Out-Null
      throw 'LibreOffice PDF export timed out after 30 seconds.'
    }
    $exportedPdf = Join-Path $OutDir ([System.IO.Path]::GetFileNameWithoutExtension($DocxPath) + '.pdf')
    if (!(Test-Path $exportedPdf)) { throw 'LibreOffice PDF export completed without producing a PDF.' }
  } finally {
    if (Test-Path $profile) { Remove-Item -LiteralPath $profile -Recurse -Force }
  }
}

function Get-PdfPageCount([string]$PdfPath) {
  if (!(Test-Path $PdfPath)) { throw "PDF export did not create $PdfPath" }
  $text = [System.Text.Encoding]::GetEncoding(28591).GetString([System.IO.File]::ReadAllBytes($PdfPath))
  return [regex]::Matches($text, '/Type\s*/Page\b').Count
}

function Test-OnePageFit([string]$DocxPath, [string]$PdfPath) {
  $pdfDir = Split-Path -Parent $PdfPath
  Export-Pdf $DocxPath $pdfDir
  $exportedPdf = Join-Path $pdfDir ([System.IO.Path]::GetFileNameWithoutExtension($DocxPath) + '.pdf')
  if ($exportedPdf -ne $PdfPath) {
    Move-Item -LiteralPath $exportedPdf -Destination $PdfPath -Force
  }
  $pageCount = Get-PdfPageCount $PdfPath
  $result = [ordered]@{
    docx_path = $DocxPath
    pdf_path = $PdfPath
    page_count = $pageCount
    fit_status = if ($pageCount -eq 1) { 'passed' } else { 'failed' }
  }
  $result | ConvertTo-Json -Compress
  if ($pageCount -ne 1) { exit 2 }
}

switch ($Command) {
  'init' { Initialize-Templates $Lang }
  'render' {
    if (!$Lang) { throw '-Lang is required for render' }
    Render-Resume $Lang $Content $Out
  }
  'validate' {
    if (!$Lang -or !$Docx) { throw '-Lang and -Docx are required for validate' }
    Validate-Resume $Lang $Docx
  }
  'fit' {
    if (!$Docx) { throw '-Docx is required for fit' }
    if (!$Out) { $Out = (Join-Path (Split-Path -Parent $Docx) ([System.IO.Path]::GetFileNameWithoutExtension($Docx) + '.pdf')) }
    Test-OnePageFit $Docx $Out
  }
  'archive' {
    if (!$Lang -or !$Variant -or !$Company -or !$Role) { throw '-Lang, -Variant, -Company, and -Role are required for archive' }
    Archive-Variant $Lang $Variant $Company $Role
  }
  'pdf' {
    if (!$Docx) { throw '-Docx is required for pdf' }
    if (!$Out) { $Out = $Generated }
    Export-Pdf $Docx $Out
  }
}
