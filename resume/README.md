# Resume Workbench

This directory keeps the resume layout stable while allowing job-specific text edits.

## Files

- `source/`: untouched original DOCX/PDF files used to create templates.
- `templates/`: protected DOCX templates generated from the originals.
- `content/base.en.json`: canonical English resume text.
- `content/base.fr.json`: canonical French resume text.
- `content/locked.en.json`: locked English section snapshots.
- `content/locked.fr.json`: locked French section snapshots.
- `generated/`: base rendered outputs.
- `variants/`: archived job-specific resume variants.
- `qa/`: validation or rendering QA artifacts.
- `scripts/ResumeWorkbench.ps1`: runnable workbench utility.

Header/contact information, section headings, and education/formation are locked in
the DOCX templates. Experience and skills are editable through JSON.

## Common Commands

Run these from the repository root.

Initialize templates and locked-section snapshots:

```powershell
powershell.exe -ExecutionPolicy Bypass -File .\resume\scripts\ResumeWorkbench.ps1 init
```

Render the base English resume:

```powershell
powershell.exe -ExecutionPolicy Bypass -File .\resume\scripts\ResumeWorkbench.ps1 render -Lang en
```

Render the base French resume:

```powershell
powershell.exe -ExecutionPolicy Bypass -File .\resume\scripts\ResumeWorkbench.ps1 render -Lang fr
```

Validate that locked sections did not change:

```powershell
powershell.exe -ExecutionPolicy Bypass -File .\resume\scripts\ResumeWorkbench.ps1 validate -Lang en -Docx .\resume\generated\Xevier_T_CV_en.generated.docx
```

Archive and render a tailored variant:

```powershell
powershell.exe -ExecutionPolicy Bypass -File .\resume\scripts\ResumeWorkbench.ps1 archive -Lang en -Variant .\resume\content\some-variant.en.json -Company "Acme" -Role "AI Engineer"
```

The Tauri `/tailor` endpoint writes job-specific `variant.json`,
`tailoring-report.json`, and DOCX files under `variants/`.

## Tailoring Rule

Do not edit `base.en.json` or `base.fr.json` for one job application. Copy the
relevant base file, tailor the copy, then archive it as a variant.
