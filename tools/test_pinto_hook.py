"""Exercise the Frappe integration: call frappe.utils.pdf.get_pinto_pdf the same way
get_print() dispatches it when pdf_generator == 'pinto'.

Usage: env/bin/python test_pinto_hook.py <html> <options.json> <out.pdf>
"""

import json
import sys

import reference


def main():
    pdfmod = reference.configure()
    # Confirm the hook is registered alongside chrome.
    hooks = pdfmod.frappe.get_hooks("pdf_generator")
    print("pdf_generator hooks:", hooks)

    html = open(sys.argv[1], encoding="utf-8").read()
    options = json.load(open(sys.argv[2])).get("options", {})
    pdf = pdfmod.get_pinto_pdf("Standard", html, options, None, "pinto")
    with open(sys.argv[3], "wb") as f:
        f.write(pdf)
    print(f"pinto via frappe hook -> {sys.argv[3]} ({len(pdf)} bytes)")


if __name__ == "__main__":
    main()
