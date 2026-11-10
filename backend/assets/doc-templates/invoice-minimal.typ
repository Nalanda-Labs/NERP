// NERP example invoice "Minimal" (Typst): monochrome, hairlines, amount due up front.
// Data: see the "Fields" list in CRM settings → Documents.
#let d = json("data.json")
#let muted = luma(120)
#let hair = 0.35pt + luma(200)

#set document(title: d.doc.title + " " + d.doc.number)
#set page(paper: "a4", margin: (x: 22mm, top: 22mm, bottom: 22mm), footer: context [
  #set text(7pt, fill: muted)
  #d.seller.legal_name
  #if d.seller.tax_id != "" [ · #d.seller.tax_id ]
  #h(1fr) #counter(page).display("1/1", both: true)
])
#set text(
  font: ("Noto Sans", "Noto Sans Devanagari", "Noto Sans Bengali", "Noto Sans Arabic", "Noto Sans Hebrew",
         "Noto Sans CJK SC", "Noto Sans CJK JP", "Noto Sans Thai", "Noto Sans Armenian", "Noto Sans Georgian"),
  size: 9pt,
  lang: d.doc.locale.split("-").at(0),
)
#set par(leading: 0.6em)
#let label(t) = text(7pt, fill: muted, tracking: 0.8pt, upper(t))

#grid(columns: (1fr, auto),
  if d.seller.has_logo { image("logo", height: 10mm) } else { text(11pt, weight: "bold", d.seller.name) },
  align(right, label(d.doc.title + " " + d.doc.number)),
)

#v(16mm)
#label[Amount due]
#v(-2pt)
#text(32pt, d.totals.total.text)

#v(12mm)
#grid(columns: (1fr, 1fr, 1fr), gutter: 14pt,
  [
    #label[Billed to] \
    #v(1pt)
    #text(weight: "bold", d.client.name) \
    #if d.client.contact_person != "" and d.client.contact_person != d.client.name [ #d.client.contact_person \ ]
    #if d.client.address != "" [ #d.client.address \ ]
    #if d.client.tax_line != "" [ #text(fill: muted, d.client.tax_line) \ ]
    #if d.client.email != "" [ #text(fill: muted, d.client.email) ]
  ],
  [
    #label[Details] \
    #v(1pt)
    #d.doc.number \
    Issued #d.doc.date \
    #text(fill: muted, d.record.title)
  ],
  [
    #label[From] \
    #v(1pt)
    #text(weight: "bold", d.seller.name) \
    #d.seller.address \
    #if d.seller.email != "" [ #text(fill: muted, d.seller.email) ]
  ],
)

#v(14mm)
#let al = (c, r) => if c >= 1 { right } else { left }
#if d.has_products {
  table(
    columns: (1fr, auto, auto, auto, auto),
    stroke: (top: none, left: none, right: none, bottom: hair),
    inset: (x, _) => (left: if x == 0 { 0pt } else { 14pt }, right: 0pt, y: 7pt),
    align: al,
    table.header(..("Description", "Qty", "Price", "Tax", "Amount").map(label)),
    ..d.products.map(p => (
      [#p.name #if p.discount != "" { text(fill: muted)[ (−#p.discount)] }],
      p.quantity + " " + p.unit, p.price.text, p.tax_rate, p.total.text,
    )).flatten(),
  )
} else {
  table(columns: (1fr, auto), stroke: (top: none, left: none, right: none, bottom: hair), inset: (x: 0pt, y: 7pt), align: (left, right), d.record.title, d.totals.total.text)
}

#v(2mm)
#align(right, block(width: 70mm, table(
  columns: (1fr, auto), stroke: none, inset: (x: 0pt, y: 3pt), align: (left, right),
  text(fill: muted)[Subtotal], d.totals.subtotal.text,
  ..if d.totals.has_discount { (text(fill: muted)[Discount], "−" + d.totals.discount.text) } else { () },
  ..d.taxes.map(t => (text(fill: muted)[Tax #t.rate], t.tax.text)).flatten(),
  table.hline(stroke: 0.6pt + black),
  text(weight: "bold")[Total], text(weight: "bold", d.totals.total.text),
)))

#v(1fr)
#grid(columns: (1fr, 1fr), gutter: 14pt,
  if d.seller.bank_details != "" [
    #label[Payment] \
    #v(1pt)
    #d.seller.bank_details
  ],
  if d.record.comments != "" [
    #label[Notes] \
    #v(1pt)
    #d.record.comments
  ],
)
