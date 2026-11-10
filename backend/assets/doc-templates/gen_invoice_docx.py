# Builds the built-in Word invoice template as clean Office Open XML.
import sys, zipfile
out = sys.argv[1]
W = 'xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"'
def run(text, bold=False, size=None, color=None, caps=False):
    rpr = ''.join([
        '<w:b/>' if bold else '', '<w:caps/>' if caps else '',
        f'<w:color w:val="{color}"/>' if color else '', f'<w:sz w:val="{size*2}"/><w:szCs w:val="{size*2}"/>' if size else ''])
    parts = text.split('\n')
    body = '<w:br/>'.join(f'<w:t xml:space="preserve">{t}</w:t>' for t in parts)
    return f'<w:r><w:rPr>{rpr}</w:rPr>{body}</w:r>'
def p(*runs, align=None, after=60):
    ppr = f'<w:pPr><w:spacing w:after="{after}"/>' + (f'<w:jc w:val="{align}"/>' if align else '') + '</w:pPr>'
    return f'<w:p>{ppr}{"".join(runs)}</w:p>'
def cell(content, width, align=None, border_bottom=None, shade=None):
    b = f'<w:tcBorders><w:bottom w:val="single" w:sz="{border_bottom[0]}" w:color="{border_bottom[1]}"/></w:tcBorders>' if border_bottom else ''
    sh = f'<w:shd w:val="clear" w:fill="{shade}"/>' if shade else ''
    paras = content if content.startswith('<w:p') else p(content, align=align, after=0)
    return f'<w:tc><w:tcPr><w:tcW w:w="{width}" w:type="dxa"/>{b}{sh}<w:vAlign w:val="center"/></w:tcPr>{paras}</w:tc>'
def table(widths, rows, borders=False):
    grid = ''.join(f'<w:gridCol w:w="{w}"/>' for w in widths)
    tb = '' if borders else '<w:tblBorders><w:top w:val="nil"/><w:left w:val="nil"/><w:bottom w:val="nil"/><w:right w:val="nil"/><w:insideH w:val="nil"/><w:insideV w:val="nil"/></w:tblBorders>'
    return (f'<w:tbl><w:tblPr><w:tblW w:w="{sum(widths)}" w:type="dxa"/>{tb}<w:tblLayout w:type="fixed"/>'
            f'<w:tblCellMar><w:left w:w="80" w:type="dxa"/><w:right w:w="80" w:type="dxa"/></w:tblCellMar></w:tblPr>'
            f'<w:tblGrid>{grid}</w:tblGrid>{"".join(rows)}</w:tbl>')
def tr(cells, header=False):
    return f'<w:tr><w:trPr>{"<w:tblHeader/>" if header else ""}<w:trHeight w:val="360"/></w:trPr>{"".join(cells)}</w:tr>'

BLUE, MUTED, LINE = '0B57D0', '6E6E6E', 'DDDDDD'
head = table([6200, 3438], [tr([
    cell(p(run('{{seller.name}}', bold=True, size=15), after=40) + p(run('{{seller.address}}', color=MUTED, size=9), after=0) + p(run('{{seller.phone}}  {{seller.email}}', color=MUTED, size=9), after=0), 6200),
    cell(p(run('{{doc.title}}', bold=True, size=22, color=BLUE, caps=True), align='right', after=40) +
         p(run('No. ', color=MUTED, size=9), run('{{doc.number}}', bold=True, size=9), align='right', after=0) +
         p(run('Date ', color=MUTED, size=9), run('{{doc.date}}', size=9), align='right', after=0), 3438),
])])
bill = (p(run('Bill to', bold=True, size=8, color=MUTED, caps=True), after=20) +
        p(run('{{client.name}}', bold=True, size=11), after=20) +
        p(run('{{client.contact_person}}', size=9), after=0) + p(run('{{client.address}}', size=9), after=0) +
        p(run('{{#client.tax_line}}{{client.tax_line}}{{/client.tax_line}}', size=9), after=20) +
        p(run('{{client.email}}  {{client.phone}}', size=9, color=MUTED), after=240))
W_ = [500, 3400, 900, 1150, 1000, 700, 1988]
hdr = tr([cell(run(h, bold=True, size=8, color=MUTED), w, align=('left' if i < 2 else 'right'), border_bottom=(12, BLUE))
          for i, (h, w) in enumerate(zip(['#', 'Item', 'Qty', 'Price', 'Discount', 'Tax', 'Amount'], W_))], header=True)
vals = ['{{#products}}{{n}}', '{{name}}', '{{quantity}} {{unit}}', '{{price}}', '{{discount}}', '{{tax_rate}}', '{{total}}{{/products}}']
row = tr([cell(run(v, size=9), w, align=('left' if i < 2 else 'right'), border_bottom=(4, LINE)) for i, (v, w) in enumerate(zip(vals, W_))])
alt = ['{{^products}}', '{{record.title}}', '', '', '', '', '{{totals.total}}{{/products}}']
none = tr([cell(run(v, size=9), w, align=('left' if i < 2 else 'right'), border_bottom=(4, LINE)) for i, (v, w) in enumerate(zip(alt, W_))])
items = table(W_, [hdr, row, none])
tot = table([5638, 2000, 2000], [
    tr([cell(run(''), 5638), cell(run('Subtotal', size=9, color=MUTED), 2000), cell(run('{{totals.subtotal}}', size=9), 2000, align='right')]),
    tr([cell(run('{{#totals.has_discount}}'), 5638), cell(run('Discount', size=9, color=MUTED), 2000), cell(run('−{{totals.discount}}{{/totals.has_discount}}', size=9), 2000, align='right')]),
    tr([cell(run('{{#totals.has_tax}}'), 5638), cell(run('Tax', size=9, color=MUTED), 2000), cell(run('{{totals.tax}}{{/totals.has_tax}}', size=9), 2000, align='right')]),
    tr([cell(run(''), 5638), cell(run('Total', bold=True, size=11), 2000, border_bottom=None), cell(run('{{totals.total}}', bold=True, size=11), 2000, align='right')]),
])
notes = (p(run(''), after=200) + p(run('Payment details', bold=True, size=8, color=MUTED, caps=True), after=20) + p(run('{{seller.bank_details}}', size=9), after=160) +
         p(run('Notes', bold=True, size=8, color=MUTED, caps=True), after=20) + p(run('{{record.comments}}', size=9)))
footer = (f'<w:ftr {W}>' + p(run('{{seller.legal_name}}   Tax ID {{seller.tax_id}}', size=7, color=MUTED), after=0) + '</w:ftr>')
body = head + p(run(''), after=200) + bill + items + p(run(''), after=120) + tot + notes
sect = ('<w:sectPr><w:footerReference w:type="default" r:id="rIdF"/><w:pgSz w:w="11906" w:h="16838"/>'
        '<w:pgMar w:top="1000" w:right="1134" w:bottom="1134" w:left="1134" w:header="500" w:footer="500"/></w:sectPr>')
doc = (f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document {W} '
       'xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">'
       f'<w:body>{body}{sect}</w:body></w:document>')
styles = (f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:styles {W}><w:docDefaults><w:rPrDefault><w:rPr>'
          '<w:rFonts w:ascii="Noto Sans" w:hAnsi="Noto Sans" w:cs="Noto Sans Arabic" w:eastAsia="Noto Sans CJK SC"/>'
          '<w:sz w:val="19"/><w:szCs w:val="19"/></w:rPr></w:rPrDefault></w:docDefaults></w:styles>')
ct = ('<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
      '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/>'
      '<Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>'
      '<Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/>'
      '<Override PartName="/word/footer1.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.footer+xml"/></Types>')
rels = ('<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
        '<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>')
drels = ('<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
         '<Relationship Id="rIdS" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/>'
         '<Relationship Id="rIdF" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/footer" Target="footer1.xml"/></Relationships>')
with zipfile.ZipFile(out, 'w', zipfile.ZIP_DEFLATED) as z:
    z.writestr('[Content_Types].xml', ct); z.writestr('_rels/.rels', rels); z.writestr('word/_rels/document.xml.rels', drels)
    z.writestr('word/document.xml', doc); z.writestr('word/styles.xml', styles); z.writestr('word/footer1.xml', footer)
