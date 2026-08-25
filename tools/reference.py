"""Generate a reference PDF using Frappe's real chrome pipeline (frappe.utils.pdf.get_chrome_pdf),
stubbing the few DB/site calls so it runs without a configured site. This is the golden output
pinto is diffed against; both use the same headless_shell binary.

Usage: env/bin/python reference.py <html> <options.json> <out.pdf> [--print-designer]
Importable: configure() -> pdf module; render_pdf(pdfmod, html, options, is_pd) -> bytes.
"""

import json
import os
import sys

BENCH = "/home/qwerty/pilot/benches/frappe-bench2"
os.environ["FRAPPE_BENCH_ROOT"] = BENCH
SITES = f"{BENCH}/sites"
CHROME = f"{BENCH}/chromium/chrome-linux/headless_shell"
HOST_URL = "http://localhost:8000/"


def configure(is_pd=False):
    """Initialize a minimal Frappe and stub the DB/site/hook calls the chrome path makes."""
    os.chdir(SITES)  # Frappe logging uses a CWD-relative ../logs path.

    import frappe

    frappe.init(site="reference.local", sites_path=SITES)
    frappe.local.lang = "en"
    frappe.local.form_dict = frappe._dict()
    frappe.local.conf = frappe._dict(frappe.local.conf or {})

    frappe.get_common_site_config()["chromium_path"] = CHROME
    frappe.get_cached_value = lambda *a, **k: is_pd
    frappe.get_installed_apps = lambda *a, **k: ["frappe"]

    _real_get_hooks = frappe.get_hooks
    hook_defaults = {
        "pdf_header_html": ["frappe.utils.pdf.pdf_header_html"],
        "pdf_footer_html": ["frappe.utils.pdf.pdf_footer_html"],
        "pdf_body_html": ["frappe.utils.pdf.pdf_body_html"],
    }

    def fake_get_hooks(name=None, *a, **k):
        if name in hook_defaults:
            return hook_defaults[name]
        try:
            return _real_get_hooks(name, *a, **k)
        except Exception:
            return []

    frappe.get_hooks = fake_get_hooks

    class DummyDB:
        def get_single_value(self, doctype, field):
            return {"pdf_page_size": "A4"}.get(field)

        def __getattr__(self, name):
            return lambda *a, **k: None

    frappe.local.db = DummyDB()

    import frappe.utils.pdf as pdfmod
    import frappe.utils.pdf_generator.browser as browsermod
    import frappe.utils.pdf_generator.page as pagemod

    pdfmod.get_host_url = lambda: HOST_URL
    browsermod.get_host_url = lambda: HOST_URL
    pagemod.get_host_url = lambda: HOST_URL

    # Render Frappe's own header/footer template with plain Jinja (no frappe calls in it),
    # so output is byte-identical without pulling in the translation/DB machinery.
    import jinja2

    def plain_header_footer(soup=None, head=None, content=None, styles=None, html_id=None, css=None, path=None):
        template_path = f"{os.path.dirname(frappe.__file__)}/{path}"
        source = open(template_path, encoding="utf-8").read()
        return jinja2.Environment().from_string(source).render(
            head=head, content=content, styles=styles, html_id=html_id, css=css,
            lang=frappe.local.lang, layout_direction="ltr",
        )

    pdfmod.pdf_header_html = plain_header_footer
    pdfmod.pdf_footer_html = plain_header_footer
    return pdfmod


def render_pdf(pdfmod, html, options, dump_dir=None):
    from frappe.utils.pdf_generator.browser import Browser
    from frappe.utils.pdf_generator.chrome_pdf_generator import ChromePDFGenerator
    from frappe.utils.pdf_generator.pdf_merge import PDFTransformer

    def write_reader(reader, path):
        from pypdf import PdfWriter

        writer = PdfWriter()
        writer.append_pages_from_reader(reader)
        with open(path, "wb") as fh:
            writer.write(fh)

    generator = ChromePDFGenerator()
    try:
        browser = Browser(generator, "Standard", html, options)
        if dump_dir:
            write_reader(browser.body_pdf, f"{dump_dir}/frappe_body.pdf")
            if getattr(browser, "header_pdf", None):
                write_reader(browser.header_pdf, f"{dump_dir}/frappe_header.pdf")
            if getattr(browser, "footer_pdf", None):
                write_reader(browser.footer_pdf, f"{dump_dir}/frappe_footer.pdf")
        transformer = PDFTransformer(browser)
        return transformer.transform_pdf(output=None)
    finally:
        generator._close_browser()


def main():
    html_path, options_path, out_path = sys.argv[1], sys.argv[2], sys.argv[3]
    is_pd = "--print-designer" in sys.argv[4:]

    pdfmod = configure(is_pd)
    html = open(html_path, encoding="utf-8").read()
    options = json.load(open(options_path)).get("options", {})

    pdf = render_pdf(pdfmod, html, options, dump_dir=os.environ.get("PINTO_DUMP"))
    with open(out_path, "wb") as f:
        f.write(pdf)
    print(f"reference written: {out_path} ({len(pdf)} bytes)")


if __name__ == "__main__":
    main()
