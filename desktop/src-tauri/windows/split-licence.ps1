# Run by installer.nsi at build time (!system). LICENSE holds the English text, a line of dashes, then the
# Arabic text. The Arabic installer shows the Arabic text first, the English one the English text first.
# OutNsh gets WASL_LICENCE_AR_CHARS, the length of the Arabic text as the licence box counts it (a line
# break is one character), so the Arabic page can make just those paragraphs right to left.
#   split-licence.ps1 <LICENSE> <out English> <out Arabic> <out .nsh>
param([string] $Licence, [string] $OutEn, [string] $OutAr, [string] $OutNsh)
$ErrorActionPreference = 'Stop'
$text = [IO.File]::ReadAllText($Licence, [Text.Encoding]::UTF8).TrimStart([char]0xFEFF)
$m = [regex]::Match($text, '(?m)^-{20,}\s*$')
if (-not $m.Success) { throw "no ---- line between the English and Arabic text in $Licence" }
$en = $text.Substring(0, $m.Index).Trim()
$ar = $text.Substring($m.Index + $m.Length).Trim()
$bom = New-Object Text.UTF8Encoding $true
[IO.File]::WriteAllText($OutEn, "$en`r`n`r`n$($m.Value.Trim())`r`n`r`n$ar`r`n", $bom)
[IO.File]::WriteAllText($OutAr, "$ar`r`n`r`n$($m.Value.Trim())`r`n`r`n$en`r`n", $bom)
[IO.File]::WriteAllText($OutNsh, "!define WASL_LICENCE_AR_CHARS $(($ar -replace "`r`n", "`n").Length)`r`n")
