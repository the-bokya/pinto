"""Generate chrome + pinto PDFs for a real doc on the live site, to compare.
Usage: env/bin/python gen_both.py <site> <doctype> <name> <out_dir>
"""

import os
import sys

BENCH = "/home/qwerty/pilot/benches/frappe-bench2"
os.environ["FRAPPE_BENCH_ROOT"] = BENCH


def main():
    site, doctype, name, out_dir = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
    os.chdir(f"{BENCH}/sites")

    import frappe

    frappe.init(site=site)
    frappe.connect()
    frappe.set_user("Administrator")
    frappe.local.form_dict = frappe._dict()

    if not frappe.db.exists(doctype, name):
        name = frappe.db.get_value(doctype, {}, "name")
        print("using doc:", name)

    for gen in ["chrome", "pinto"]:
        try:
            pdf = frappe.get_print(doctype, name, "Standard", as_pdf=True, no_letterhead=1, pdf_generator=gen)
            path = f"{out_dir}/todo_{gen}.pdf"
            with open(path, "wb") as f:
                f.write(pdf)
            print(f"{gen}: {path} ({len(pdf)} bytes)")
        except Exception as e:
            print(f"{gen}: ERROR {type(e).__name__}: {e}")
            import traceback
            traceback.print_exc()

    frappe.destroy()


if __name__ == "__main__":
    main()
