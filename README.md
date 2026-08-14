# Resume Workbench

This directory keeps the resume layout stable while allowing job-specific text edits.

## Files

- `source/`: untouched originals copied from the workspace root.
- `templates/`: protected DOCX templates generated from the originals.
- `content/base.en.json`: canonical English resume text.
- `content/base.fr.json`: canonical French resume text.
- `generated/`: base rendered outputs.
- `variants/`: archived job-specific resume variants.
- `scripts/ResumeWorkbench.ps1`: runnable workbench utility.

Header/contact information, section headings, and education/formation are locked in
the DOCX templates. Experience and skills are editable through JSON.

## Common Commands

Initialize templates and locked-section snapshots:

```powershell
powershell.exe -ExecutionPolicy Bypass -File .\scripts\ResumeWorkbench.ps1 init
```

Render the base English resume:

```powershell
powershell.exe -ExecutionPolicy Bypass -File .\scripts\ResumeWorkbench.ps1 render -Lang en
```

Render the base French resume:

```powershell
powershell.exe -ExecutionPolicy Bypass -File .\scripts\ResumeWorkbench.ps1 render -Lang fr
```

Validate that locked sections did not change:

```powershell
powershell.exe -ExecutionPolicy Bypass -File .\scripts\ResumeWorkbench.ps1 validate -Lang en -Docx .\generated\Xevier_T_CV_en.generated.docx
```

Archive and render a tailored variant:

```powershell
powershell.exe -ExecutionPolicy Bypass -File .\scripts\ResumeWorkbench.ps1 archive -Lang en -Variant .\content\some-variant.en.json -Company "Acme" -Role "AI Engineer"
```

## Tailoring Rule

Do not edit `base.en.json` or `base.fr.json` for one job application. Copy the
relevant base file, tailor the copy, then archive it as a variant.
