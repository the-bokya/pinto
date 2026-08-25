# Using pinto as a Frappe PDF Generator

pinto plugs into Frappe's `pdf_generator` mechanism exactly like the built-in `chrome`
generator, so it appears in the **PDF Generator** dropdown (Print Settings and each Print
Format) and is used whenever a PDF is downloaded/attached.

Frappe dispatches PDF generation in `frappe/utils/print_utils.py::get_print`: if the selected
generator isn't `wkhtmltopdf`, it calls each `pdf_generator` hook until one returns bytes. We add
a hook that owns `"pinto"` and shells out to the `pinto` binary with the same printview HTML +
options the `chrome` generator receives.

## Prerequisites

```bash
cd ~/pinto && cargo build --release      # produces target/release/pinto
```

The hook finds the binary via, in order: site config `pinto_path` → `pinto` on `PATH` →
`~/pinto/target/release/pinto`. To pin it explicitly:

```bash
# sites/common_site_config.json (or a site's site_config.json)
{ "pinto_path": "/absolute/path/to/pinto/target/release/pinto" }
```

## Changes in `apps/frappe`

### 1. Register the hook — `frappe/hooks.py`

```python
# was: pdf_generator = "frappe.utils.pdf.get_chrome_pdf"
pdf_generator = ["frappe.utils.pdf.get_chrome_pdf", "frappe.utils.pdf.get_pinto_pdf"]
```

### 2. The generator function — `frappe/utils/pdf.py`

```python
def get_pinto_pdf(print_format, html, options, output=None, pdf_generator=None):
	"""Render via the standalone `pinto` binary: in-process HTML->PDF, no browser.

	pinto ingests the same printview HTML + options that the chrome generator gets and
	produces the PDF itself. The binary path comes from site config `pinto_path`, else
	`pinto` on PATH, else the default build location.
	"""
	if pdf_generator != "pinto":
		return

	import json
	import os
	import shutil
	import subprocess
	import tempfile

	binary = (
		frappe.get_common_site_config().get("pinto_path")
		or shutil.which("pinto")
		or os.path.expanduser("~/pinto/target/release/pinto")
	)
	if not os.path.exists(binary):
		frappe.throw(_("pinto binary not found at {0}. Set `pinto_path` in site config.").format(binary))

	is_pd = bool(frappe.get_cached_value("Print Format", print_format, "print_designer")) if print_format else False
	config = {
		"options": options or {},
		"is_print_designer": is_pd,
		"host_url": get_host_url(),
		"sid": getattr(getattr(frappe, "session", None), "sid", None),
		"bench_sites_path": os.path.join(frappe.utils.get_bench_path(), "sites"),
		"site_public_path": frappe.utils.get_site_path("public"),
		"default_page_size": frappe.db.get_single_value("Print Settings", "pdf_page_size") or "A4",
	}

	with tempfile.TemporaryDirectory() as tmp:
		html_path = os.path.join(tmp, "input.html")
		config_path = os.path.join(tmp, "config.json")
		out_path = os.path.join(tmp, "output.pdf")
		with open(html_path, "w", encoding="utf-8") as f:
			f.write(html)
		with open(config_path, "w", encoding="utf-8") as f:
			json.dump(config, f)
		try:
			subprocess.run(
				[binary, "--html", html_path, "--options", config_path, "--out", out_path],
				check=True,
				capture_output=True,
				text=True,
			)
		except subprocess.CalledProcessError as e:
			frappe.throw(_("pinto failed: {0}").format(e.stderr or e.stdout or str(e)))
		with open(out_path, "rb") as f:
			return f.read()
```

The `config` is exactly the JSON schema the pinto CLI expects (`--options`). `bench_sites_path`
/ `site_public_path` let pinto resolve host-relative `<img>` and `<link>` (e.g.
`/assets/frappe/dist/css/print.bundle.css`, so Bootstrap + the print styles apply).

### 3. Add "pinto" to the dropdown

Add `\npinto` to the `pdf_generator` Select field options:

- `frappe/printing/doctype/print_format/print_format.json` — `"options": "wkhtmltopdf\nchrome\npinto"`
- `frappe/printing/doctype/print_settings/print_settings.json` — same

And widen the type hints (optional, cosmetic):

- `print_format/print_format.py`: `pdf_generator: DF.Literal["wkhtmltopdf", "chrome", "pinto"]`
- `frappe/utils/print_utils.py` and `frappe/utils/print_format.py`: `Literal["wkhtmltopdf", "chrome", "pinto"]`

No JS changes are needed — `printing/page/print/print.js` passes whatever generator is set
straight through to `download_pdf`.

## Activate & use

```bash
# 1. build the frontend assets if the site was never built (needed for print.bundle.css)
node apps/frappe/esbuild --production --apps frappe        # from the bench root

# 2. pick up the new hook + field option
bench --site <site> clear-cache          # or: pilot frappe --site <site> clear-cache
bench --site <site> migrate              # syncs the Select option to the DB
# then restart the web workers if running
```

In the UI, set **PDF Generator → pinto** either globally (Print Settings) or on a specific Print
Format, then **Download PDF** on any document. It renders in-process — no Chrome, ~10 MB RAM.

## Notes

- **Fidelity**: for real formats (which pull in `print.bundle.css` + Bootstrap grid) pinto renders
  ~0.7% (fuzzed) from chrome — visually near-identical. It is not pixel-identical (different
  rasterizer). Use the `chrome` generator when byte-for-byte chrome parity is required.
- These are edits to Frappe core; package them as a small app hook instead if you don't want to
  patch the vendored framework (`pdf_generator` hooks compose across installed apps).
