# Builds the three example Word invoices (Classic, Modern, Minimal) as clean Office Open XML.
#   python3 gen_example_docx.py <output dir>
# Placeholder rules (see src/crm/docs/office.rs):
#   * a paragraph holding {{…}} is collapsed into its first run, so each placeholder whose
#     formatting matters gets its own paragraph or table cell;
#   * {{#products}} in a row's first cell and {{/products}} in its last repeat the row;
#   * the same pair around rows or a whole paragraph keeps them only when the value is set.
import os, sys, zipfile

W = 'xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"'
R = 'xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"'
MM = 56.7  # twips per millimetre


def esc(t):
    return t.replace('&', '&amp;').replace('<', '&lt;').replace('>', '&gt;')


def run(text, bold=False, size=None, color=None, caps=False, smallcaps=False, spacing=None, font=None):
    rpr = ''.join([
        f'<w:rFonts w:ascii="{font}" w:hAnsi="{font}"/>' if font else '',
        '<w:b/><w:bCs/>' if bold else '', '<w:caps/>' if caps else '', '<w:smallCaps/>' if smallcaps else '',
        f'<w:color w:val="{color}"/>' if color else '', f'<w:spacing w:val="{spacing}"/>' if spacing else '',
        f'<w:sz w:val="{int(size * 2)}"/><w:szCs w:val="{int(size * 2)}"/>' if size else ''])
    return f'<w:r><w:rPr>{rpr}</w:rPr><w:t xml:space="preserve">{esc(text)}</w:t></w:r>'


def p(*runs, align=None, before=0, after=40, border=None, keep=False):
    """border: dict side -> (val, size in eighths of a point, colour, space)."""
    bdr = ''
    if border:
        bdr = '<w:pBdr>' + ''.join(f'<w:{s} w:val="{v}" w:sz="{sz}" w:space="{sp}" w:color="{c}"/>' for s, (v, sz, c, sp) in border.items()) + '</w:pBdr>'
    ppr = (f'<w:pPr>{"<w:keepNext/>" if keep else ""}{bdr}<w:spacing w:before="{before}" w:after="{after}"/>'
           + (f'<w:jc w:val="{align}"/>' if align else '') + '</w:pPr>')
    return f'<w:p>{ppr}{"".join(runs)}</w:p>'


def gap(twips):
    return f'<w:p><w:pPr><w:spacing w:before="0" w:after="0" w:line="{max(twips, 20)}" w:lineRule="exact"/></w:pPr></w:p>'


def cell(content, width, shade=None, borders=None, valign='top', vmerge=None, margins=None, span=None):
    """content: paragraphs XML (or a run, which is wrapped). borders: dict side -> (sz, colour[, val])."""
    if not content.startswith('<w:p'):
        content = p(content, after=0)
    b = ''
    if borders:
        b = '<w:tcBorders>' + ''.join(
            f'<w:{s} w:val="{(v[2] if len(v) > 2 else "single")}" w:sz="{v[0]}" w:space="0" w:color="{v[1]}"/>' for s, v in borders.items()) + '</w:tcBorders>'
    mar = ''
    if margins:
        mar = '<w:tcMar>' + ''.join(f'<w:{s} w:w="{v}" w:type="dxa"/>' for s, v in margins.items()) + '</w:tcMar>'
    vm = '' if vmerge is None else ('<w:vMerge w:val="restart"/>' if vmerge == 'restart' else '<w:vMerge/>')
    gs = f'<w:gridSpan w:val="{span}"/>' if span else ''
    sh = f'<w:shd w:val="clear" w:color="auto" w:fill="{shade}"/>' if shade else ''
    return f'<w:tc><w:tcPr><w:tcW w:w="{width}" w:type="dxa"/>{gs}{vm}{b}{sh}{mar}<w:vAlign w:val="{valign}"/></w:tcPr>{content}</w:tc>'


def tr(cells, header=False, height=None, exact=False):
    h = f'<w:trHeight w:val="{height}" w:hRule="{"exact" if exact else "atLeast"}"/>' if height else ''
    return f'<w:tr><w:trPr><w:cantSplit/>{"<w:tblHeader/>" if header else ""}{h}</w:trPr>{"".join(cells)}</w:tr>'


def table(widths, rows, align=None, indent=None, cell_mar=(80, 80)):
    grid = ''.join(f'<w:gridCol w:w="{w}"/>' for w in widths)
    nil = ''.join(f'<w:{s} w:val="nil"/>' for s in ('top', 'left', 'bottom', 'right', 'insideH', 'insideV'))
    return (f'<w:tbl><w:tblPr><w:tblW w:w="{sum(widths)}" w:type="dxa"/>'
            + (f'<w:jc w:val="{align}"/>' if align else '')
            + (f'<w:tblInd w:w="{indent}" w:type="dxa"/>' if indent is not None else '')
            + f'<w:tblBorders>{nil}</w:tblBorders><w:tblLayout w:type="fixed"/>'
            f'<w:tblCellMar><w:top w:w="0" w:type="dxa"/><w:left w:w="{cell_mar[0]}" w:type="dxa"/>'
            f'<w:bottom w:w="0" w:type="dxa"/><w:right w:w="{cell_mar[1]}" w:type="dxa"/></w:tblCellMar></w:tblPr>'
            f'<w:tblGrid>{grid}</w:tblGrid>{"".join(rows)}</w:tbl>')


def cond(path, *runs_or_text, **kw):
    """A paragraph kept only when `path` is set. The first run carries the paragraph's look."""
    first = runs_or_text[0]
    return p(first.replace('<w:t xml:space="preserve">', f'<w:t xml:space="preserve">{{{{#{path}}}}}', 1)
             .replace('</w:t></w:r>', f'{{{{/{path}}}}}</w:t></w:r>', 1), **kw)


def page_fields(size, color, align, prefix='Page ', middle=' of '):
    def fld(instr):
        return (f'<w:r><w:rPr><w:color w:val="{color}"/><w:sz w:val="{size * 2}"/></w:rPr><w:fldChar w:fldCharType="begin"/></w:r>'
                f'<w:r><w:rPr><w:color w:val="{color}"/><w:sz w:val="{size * 2}"/></w:rPr><w:instrText xml:space="preserve"> {instr} </w:instrText></w:r>'
                f'<w:r><w:rPr><w:color w:val="{color}"/><w:sz w:val="{size * 2}"/></w:rPr><w:fldChar w:fldCharType="separate"/></w:r>'
                f'<w:r><w:rPr><w:color w:val="{color}"/><w:sz w:val="{size * 2}"/></w:rPr><w:t>1</w:t></w:r>'
                f'<w:r><w:rPr><w:color w:val="{color}"/><w:sz w:val="{size * 2}"/></w:rPr><w:fldChar w:fldCharType="end"/></w:r>')
    return p(run(prefix, size=size, color=color), fld('PAGE'), run(middle, size=size, color=color), fld('NUMPAGES'), align=align, after=0)


def package(path, body, footer, font, cs_font='Noto Sans Arabic', header=None, margins=(1134, 1134, 1134, 1134), header_dist=500, size=9.5):
    top, right, bottom, left = margins
    sect = ('<w:sectPr>' + ('<w:headerReference w:type="default" r:id="rIdH"/>' if header else '')
            + '<w:footerReference w:type="default" r:id="rIdF"/><w:pgSz w:w="11906" w:h="16838"/>'
            f'<w:pgMar w:top="{top}" w:right="{right}" w:bottom="{bottom}" w:left="{left}" w:header="{header_dist}" w:footer="500" w:gutter="0"/></w:sectPr>')
    doc = (f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document {W} {R}>'
           f'<w:body>{body}{sect}</w:body></w:document>')
    styles = (f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:styles {W}><w:docDefaults><w:rPrDefault><w:rPr>'
              f'<w:rFonts w:ascii="{font}" w:hAnsi="{font}" w:cs="{cs_font}" w:eastAsia="Noto Sans CJK SC"/>'
              f'<w:sz w:val="{int(size * 2)}"/><w:szCs w:val="{int(size * 2)}"/></w:rPr></w:rPrDefault>'
              '<w:pPrDefault><w:pPr><w:spacing w:after="0" w:line="264" w:lineRule="auto"/></w:pPr></w:pPrDefault></w:docDefaults></w:styles>')
    over = ('<Override PartName="/word/header1.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.header+xml"/>' if header else '')
    ct = ('<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
          '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/>'
          '<Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>'
          '<Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/>'
          f'{over}<Override PartName="/word/footer1.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.footer+xml"/></Types>')
    rels = ('<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
            '<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>')
    hrel = ('<Relationship Id="rIdH" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/header" Target="header1.xml"/>' if header else '')
    drels = ('<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
             '<Relationship Id="rIdS" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/>'
             f'{hrel}<Relationship Id="rIdF" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/footer" Target="footer1.xml"/></Relationships>')
    with zipfile.ZipFile(path, 'w', zipfile.ZIP_DEFLATED) as z:
        for name, data in [('[Content_Types].xml', ct), ('_rels/.rels', rels), ('word/_rels/document.xml.rels', drels),
                           ('word/document.xml', doc), ('word/styles.xml', styles),
                           ('word/footer1.xml', f'<w:ftr {W} {R}>{footer}</w:ftr>')] + ([('word/header1.xml', f'<w:hdr {W} {R}>{header}</w:hdr>')] if header else []):
            # Fixed timestamps: the same source always gives byte-identical files.
            z.writestr(zipfile.ZipInfo(name, date_time=(2026, 1, 1, 0, 0, 0)), data, compress_type=zipfile.ZIP_DEFLATED)


TEXT_W = 11906 - 2 * 1134  # A4 width minus 20 mm margins


# ---------------------------------------------------------------------------
# Classic: serif letterhead, ruled table, signature line
# ---------------------------------------------------------------------------
def classic(out):
    INK, MUTED, RULE, HEAD = '1F3A5F', '666666', 'BBBBBB', 'E3E8EF'
    tw = TEXT_W

    def lbl(t):
        return run(t, size=7.5, color=MUTED, caps=True, spacing=30)

    letterhead = (
        p(run('{{seller.name}}', size=21, color=INK, smallcaps=True), align='center', after=20) +
        p(run('{{seller.address}}', size=8.5, color=MUTED), align='center', after=0) +
        p(run('{{seller.phone}}   {{seller.email}}   {{seller.website}}', size=8.5, color=MUTED), align='center', after=60,
          border={'bottom': ('double', 6, INK, 4)}) +
        p(run(''), after=280) +
        p(run('{{doc.title}}', size=15, color=INK, caps=True, spacing=80), align='center', after=360))

    billed = (p(lbl('Billed to'), after=60) +
              p(run('{{client.name}}', bold=True, size=11.5), after=20) +
              cond('client.contact_person', run('Attn. {{client.contact_person}}'), after=0) +
              cond('client.address', run('{{client.address}}'), after=0) +
              cond('client.tax_line', run('{{client.tax_line}}'), after=0) +
              p(run('{{client.email}}   {{client.phone}}', color=MUTED), after=0))
    L, K, V = 5000, 1500, tw - 6500
    details = [('Number', run('{{doc.number}}', bold=True)), ('Date', run('{{doc.date}}')),
               ('Reference', run('{{record.title}}')), ('Prepared by', run('{{record.responsible}}'))]
    info_rows = [tr([cell(billed, L, vmerge='restart'), cell(lbl('Invoice details'), K + V, span=2)])]
    for k, v in details:
        info_rows.append(tr([cell(p(run(''), after=0), L, vmerge='continue'),
                             cell(run(k, color=MUTED), K), cell(v, V)]))
    info = table([L, K, V], info_rows)

    ws = [600, 3200, 900, 1250, 800, 700, 2188]
    ws[-1] = tw - sum(ws[:-1])
    border = {s: (4, RULE) for s in ('top', 'left', 'bottom', 'right')}
    heads = ['No.', 'Description', 'Qty', 'Unit price', 'Disc.', 'Tax', 'Amount']
    hdr = tr([cell(p(run(h, bold=True, size=8.5, color=INK), align='left' if i < 2 else 'right', after=0), w, shade=HEAD, borders=border, valign='center')
              for i, (h, w) in enumerate(zip(heads, ws))], header=True, height=380)
    vals = ['{{#products}}{{n}}', '{{name}}', '{{quantity}} {{unit}}', '{{price}}', '{{discount}}', '{{tax_rate}}', '{{total}}{{/products}}']
    row = tr([cell(p(run(v), align='left' if i < 2 else 'right', after=0), w, borders=border, valign='center')
              for i, (v, w) in enumerate(zip(vals, ws))], height=380)
    alt = ['{{^products}}', '{{record.title}}', '', '', '', '', '{{totals.total}}{{/products}}']
    none = tr([cell(p(run(v), align='left' if i < 2 else 'right', after=0), w, borders=border, valign='center')
               for i, (v, w) in enumerate(zip(alt, ws))], height=380)
    items = table(ws, [hdr, row, none])

    a, b = 2600, 1900
    def trow(label, value, flag=None, **kw):
        lr, vr = run(label), run(value)
        if flag:
            lr = run('{{#' + flag + '}}' + label)
            vr = run(value + '{{/' + flag + '}}')
        return tr([cell(p(lr, after=0), a, **kw), cell(p(vr, align='right', after=0), b, **kw)], height=320)
    totals = table([a, b], [
        trow('Subtotal', '{{totals.subtotal}}'),
        trow('Discount', '−{{totals.discount}}', 'totals.has_discount'),
        trow('Tax', '{{totals.tax}}', 'totals.has_tax'),
        tr([cell(p(run('Total due', bold=True, size=12, color=INK), after=0), a, borders={'top': (16, INK), 'bottom': (4, INK)}, valign='center'),
            cell(p(run('{{totals.total}}', bold=True, size=12, color=INK), align='right', after=0), b, borders={'top': (16, INK), 'bottom': (4, INK)}, valign='center')], height=440),
    ], align='right')

    notes = (cond('seller.bank_details', lbl('Payment details'), after=40) +
             cond('seller.bank_details', run('{{seller.bank_details}}'), after=200) +
             cond('record.comments', lbl('Terms and notes'), after=40) +
             cond('record.comments', run('{{record.comments}}'), after=0) + p(run(''), after=0))
    sign = (p(run(''), after=900) +
            p(run('Authorised signatory · {{seller.name}}', size=8.5, color=MUTED), align='center', after=0, border={'top': ('single', 4, MUTED, 4)}))
    bottom = table([tw - 3600, 3600], [tr([cell(notes, tw - 3600, valign='bottom', margins={'right': 400}), cell(sign, 3600, valign='bottom')])])

    body = letterhead + info + p(run(''), after=360) + items + p(run(''), after=160) + totals + p(run(''), after=500) + bottom
    footer = (p(run('{{seller.legal_name}}   Tax ID {{seller.tax_id}}   {{seller.registration_no}}', size=7.5, color=MUTED), align='center', after=0) +
              page_fields(7, MUTED, 'center'))
    package(out, body, footer, 'Liberation Serif', margins=(1020, 1134, 1250, 1134), size=10)


# ---------------------------------------------------------------------------
# Modern: colour band header (repeats per page), summary cards, bold totals
# ---------------------------------------------------------------------------
def modern(out):
    ACC, SOFT, CARD, MUTED, LINE = '00695C', 'E0F2F1', 'F3F3F3', '6B6B6B', 'E6E6E6'
    margin = 907  # 16 mm
    tw = 11906 - 2 * margin

    def lbl(t, color=MUTED):
        return run(t, bold=True, size=7.5, color=color, caps=True, spacing=10)

    # Full-bleed band: the header table is pulled out to the page edges.
    # Spacer columns stand in for the page margins inside the band.
    lw, rw = 6000, 11906 - 6000 - 2 * margin
    edge = lambda: cell(p(run(''), after=0), margin, shade=ACC)
    band = table([margin, lw, rw, margin], [tr([
        edge(),
        cell(p(run('{{seller.name}}', bold=True, size=18, color='FFFFFF'), after=60) +
             p(run('{{seller.address}}', size=8.5, color='DDEEEC'), after=0) +
             p(run('{{seller.phone}}   {{seller.email}}   {{seller.website}}', size=8.5, color='DDEEEC'), after=0),
             lw, shade=ACC, valign='center'),
        cell(p(run('{{doc.title}}', bold=True, size=30, color='FFFFFF', caps=True), align='right', after=0) +
             p(run('{{doc.number}}', size=11, color='FFFFFF'), align='right', after=0),
             rw, shade=ACC, valign='center'),
        edge(),
    ], height=2550, exact=True)], indent=-margin, cell_mar=(0, 0))
    header = band + p(run(''), after=0)

    g = 200
    cw = (tw - 2 * g) // 3
    c3 = tw - 2 * g - 2 * cw
    pad = {'top': 160, 'bottom': 160, 'left': 180, 'right': 180}
    cards = table([cw, g, cw, g, c3], [tr([
        cell(p(lbl('Issued'), after=40) + p(run('{{doc.date}}', bold=True, size=11), after=0), cw, shade=CARD, margins=pad),
        cell(p(run(''), after=0), g),
        cell(p(lbl('Reference'), after=40) + p(run('{{record.title}}', bold=True, size=10), after=0), cw, shade=CARD, margins=pad),
        cell(p(run(''), after=0), g),
        cell(p(lbl('Amount due'), after=40) + p(run('{{totals.total}}', bold=True, size=15, color=ACC), after=0), c3, shade=SOFT, margins=pad),
    ])], cell_mar=(0, 0))

    half = tw // 2
    parties = table([half, tw - half], [tr([
        cell(p(lbl('Billed to'), after=40) + p(run('{{client.name}}', bold=True, size=11), after=20) +
             cond('client.contact_person', run('{{client.contact_person}}'), after=0) +
             cond('client.address', run('{{client.address}}'), after=0) +
             cond('client.tax_line', run('{{client.tax_line}}'), after=0) +
             p(run('{{client.email}}   {{client.phone}}', color=MUTED), after=0), half),
        cell(p(lbl('Contact'), after=40) + p(run('{{record.responsible}}'), after=0) + p(run('{{seller.email}}', color=MUTED), after=0), tw - half),
    ])], cell_mar=(0, 0))

    ws = [500, 3450, 950, 1200, 1250, 700, 0]
    ws[-1] = tw - sum(ws[:-1])
    heads = ['#', 'Item', 'Qty', 'Price', 'Discount', 'Tax', 'Amount']
    pad2 = {'left': 120, 'right': 120}
    hdr = tr([cell(p(lbl(h, 'FFFFFF'), align='left' if i < 2 else 'right', after=0), w, shade=ACC, valign='center', margins=pad2)
              for i, (h, w) in enumerate(zip(heads, ws))], header=True, height=440)
    vals = ['{{#products}}{{n}}', '{{name}}', '{{quantity}} {{unit}}', '{{price}}', '{{discount}}', '{{tax_rate}}', '{{total}}{{/products}}']
    row = tr([cell(p(run(v, bold=(i == 6)), align='left' if i < 2 else 'right', after=0), w, valign='center', borders={'bottom': (6, LINE)}, margins=pad2)
              for i, (v, w) in enumerate(zip(vals, ws))], height=440)
    alt = ['{{^products}}', '{{record.title}}', '', '', '', '', '{{totals.total}}{{/products}}']
    none = tr([cell(p(run(v, bold=(i == 6)), align='left' if i < 2 else 'right', after=0), w, valign='center', borders={'bottom': (6, LINE)}, margins=pad2)
               for i, (v, w) in enumerate(zip(alt, ws))], height=440)
    items = table(ws, [hdr, row, none], cell_mar=(0, 0))

    a, b = 2700, 2000
    def trow(label, value, flag=None):
        lr, vr = run(label, color=MUTED), run(value)
        if flag:
            lr = run('{{#' + flag + '}}' + label, color=MUTED)
            vr = run(value + '{{/' + flag + '}}')
        return tr([cell(p(lr, after=0), a, margins=pad2), cell(p(vr, align='right', after=0), b, margins=pad2)], height=300)
    totals = table([a, b], [
        trow('Subtotal', '{{totals.subtotal}}'),
        trow('Discount', '−{{totals.discount}}', 'totals.has_discount'),
        trow('Tax', '{{totals.tax}}', 'totals.has_tax'),
        tr([cell(p(run('Total', bold=True, size=10, color='FFFFFF'), after=0), a, shade=ACC, valign='center', margins={'left': 200}),
            cell(p(run('{{totals.total}}', bold=True, size=14, color='FFFFFF'), align='right', after=0), b, shade=ACC, valign='center', margins={'right': 200})],
           height=560),
    ], align='right', cell_mar=(0, 0))

    bar = {'left': ('single', 24, ACC, 8)}
    pay = (cond('seller.bank_details', lbl('How to pay'), after=40, border=bar) +
           cond('seller.bank_details', run('{{seller.bank_details}}'), after=240, border=bar) +
           cond('record.comments', lbl('Notes'), after=40) +
           cond('record.comments', run('{{record.comments}}'), after=0))
    thanks = p(run('Thank you for your business!', bold=True, size=10, color=ACC), align='center', before=360, after=0)

    body = cards + p(run(''), after=280) + parties + p(run(''), after=280) + items + p(run(''), after=160) + totals + p(run(''), after=400) + pay + thanks
    footer = table([tw - 2600, 2600], [tr([
        cell(p(run('{{seller.legal_name}}   Tax ID {{seller.tax_id}}   {{seller.registration_no}}', size=7.5, color=MUTED), after=0), tw - 2600, borders={'top': (4, LINE)}),
        cell(page_fields(7.5, MUTED, 'right', prefix='', middle=' / '), 2600, borders={'top': (4, LINE)}),
    ])], cell_mar=(0, 0))
    package(out, body, footer, 'Noto Sans', header=header, margins=(1920 + 400, margin, 1134, margin), header_dist=0)


# ---------------------------------------------------------------------------
# Minimal: monochrome, hairlines, amount due up front
# ---------------------------------------------------------------------------
def minimal(out):
    MUTED, HAIR = '7A7A7A', 'C8C8C8'
    margin = 1247  # 22 mm
    tw = 11906 - 2 * margin

    def lbl(t):
        return run(t, size=6.5, color=MUTED, caps=True, spacing=16)

    top = table([tw // 2, tw - tw // 2], [tr([
        cell(p(run('{{seller.name}}', bold=True, size=11), after=0), tw // 2),
        cell(p(run('{{doc.title}} {{doc.number}}', size=7, color=MUTED, caps=True, spacing=16), align='right', after=0), tw - tw // 2, valign='center'),
    ])], cell_mar=(0, 0))
    due = p(lbl('Amount due'), before=900, after=0) + p(run('{{totals.total}}', size=32), after=0)

    g = 280
    c = (tw - 2 * g) // 3
    c3 = tw - 2 * g - 2 * c
    blank = cell(p(run(''), after=0), g)
    parties = table([c, g, c, g, c3], [tr([
        cell(p(lbl('Billed to'), after=60) + p(run('{{client.name}}', bold=True), after=0) +
             cond('client.contact_person', run('{{client.contact_person}}'), after=0) +
             cond('client.address', run('{{client.address}}'), after=0) +
             cond('client.tax_line', run('{{client.tax_line}}', color=MUTED), after=0) +
             cond('client.email', run('{{client.email}}', color=MUTED), after=0), c),
        blank,
        cell(p(lbl('Details'), after=60) + p(run('{{doc.number}}'), after=0) + p(run('Issued {{doc.date}}'), after=0) +
             p(run('{{record.title}}', color=MUTED), after=0), c),
        blank,
        cell(p(lbl('From'), after=60) + p(run('{{seller.name}}', bold=True), after=0) + p(run('{{seller.address}}'), after=0) +
             cond('seller.email', run('{{seller.email}}', color=MUTED), after=0), c3),
    ])], cell_mar=(0, 0))

    ws = [4300, 1100, 1350, 900, 0]
    ws[-1] = tw - sum(ws[:-1])
    hair = {'bottom': (4, HAIR)}
    heads = ['Description', 'Qty', 'Price', 'Tax', 'Amount']
    hdr = tr([cell(p(lbl(h), align='left' if i == 0 else 'right', after=0), w, borders=hair, valign='bottom') for i, (h, w) in enumerate(zip(heads, ws))],
             header=True, height=400)
    vals = ['{{#products}}{{name}}', '{{quantity}} {{unit}}', '{{price}}', '{{tax_rate}}', '{{total}}{{/products}}']
    row = tr([cell(p(run(v), align='left' if i == 0 else 'right', after=0), w, borders=hair, valign='center') for i, (v, w) in enumerate(zip(vals, ws))],
             height=500)
    alt = ['{{^products}}{{record.title}}', '', '', '', '{{totals.total}}{{/products}}']
    none = tr([cell(p(run(v), align='left' if i == 0 else 'right', after=0), w, borders=hair, valign='center') for i, (v, w) in enumerate(zip(alt, ws))],
              height=500)
    items = table(ws, [hdr, row, none], cell_mar=(0, 0))

    a, b = 2200, 1800
    def trow(label, value, flag=None):
        lr, vr = run(label, color=MUTED), run(value)
        if flag:
            lr = run('{{#' + flag + '}}' + label, color=MUTED)
            vr = run(value + '{{/' + flag + '}}')
        return tr([cell(p(lr, after=0), a), cell(p(vr, align='right', after=0), b)], height=300)
    totals = table([a, b], [
        trow('Subtotal', '{{totals.subtotal}}'),
        trow('Discount', '−{{totals.discount}}', 'totals.has_discount'),
        trow('Tax', '{{totals.tax}}', 'totals.has_tax'),
        tr([cell(p(run('Total', bold=True), after=0), a, borders={'top': (6, '000000')}, valign='center'),
            cell(p(run('{{totals.total}}', bold=True), align='right', after=0), b, borders={'top': (6, '000000')}, valign='center')], height=360),
    ], align='right', cell_mar=(0, 0))

    half = (tw - g) // 2
    end = table([half, g, tw - g - half], [tr([
        cell(cond('seller.bank_details', lbl('Payment'), after=60) + cond('seller.bank_details', run('{{seller.bank_details}}'), after=0) + p(run(''), after=0), half),
        blank,
        cell(cond('record.comments', lbl('Notes'), after=60) + cond('record.comments', run('{{record.comments}}'), after=0) + p(run(''), after=0), tw - g - half),
    ])], cell_mar=(0, 0))

    body = top + due + p(run(''), after=600) + parties + p(run(''), after=700) + items + p(run(''), after=120) + totals + p(run(''), after=1400) + end
    footer = table([tw - 1500, 1500], [tr([
        cell(p(run('{{seller.legal_name}}   {{seller.tax_id}}', size=7, color=MUTED), after=0), tw - 1500),
        cell(page_fields(7, MUTED, 'right', prefix='', middle='/'), 1500),
    ])], cell_mar=(0, 0))
    package(out, body, footer, 'Noto Sans', margins=(margin, margin, margin, margin), size=9)


if __name__ == '__main__':
    d = sys.argv[1] if len(sys.argv) > 1 else os.path.dirname(os.path.abspath(__file__))
    classic(os.path.join(d, 'invoice-classic.docx'))
    modern(os.path.join(d, 'invoice-modern.docx'))
    minimal(os.path.join(d, 'invoice-minimal.docx'))
