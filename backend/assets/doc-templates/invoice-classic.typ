// NERP example invoice "Classic" (Typst): serif letterhead, ruled table, signature line.
// Data: see the "Fields" list in CRM settings → Documents.
#let d = json("data.json")
#let ink = rgb("#1f3a5f")
#let muted = luma(100)
#let rule = luma(185)

#set document(title: d.doc.title + " " + d.doc.number)
#set page(paper: "a4", margin: (x: 20mm, top: 18mm, bottom: 22mm), footer: context [
  #set text(7.5pt, fill: muted)
  #align(center)[
    #d.seller.legal_name
    #if d.seller.tax_id != "" [ · Tax ID #d.seller.tax_id ]
    #if d.seller.registration_no != "" [ · #d.seller.registration_no ]
    · Page #counter(page).display("1 of 1", both: true)
  ]
])
// Serif first; the Noto chain covers every other script in customer data.
#set text(
  font: ("Libertinus Serif", "Noto Serif", "Noto Sans", "Noto Sans Devanagari", "Noto Sans Bengali", "Noto Sans Arabic",
         "Noto Sans Hebrew", "Noto Sans CJK SC", "Noto Sans CJK JP", "Noto Sans Thai", "Noto Sans Armenian", "Noto Sans Georgian"),
  size: 10pt,
  lang: d.doc.locale.split("-").at(0),
)
#set par(leading: 0.6em)
#let label(t) = text(7.5pt, fill: muted, tracking: 1.2pt, upper(t))

// Letterhead
#align(center)[
  #if d.seller.has_logo { image("logo", height: 16mm); v(3pt) }
  #text(21pt, fill: ink, smallcaps(d.seller.name)) \
  #v(1pt)
  #text(8.5pt, fill: muted)[
    #d.seller.address \
    #d.seller.phone #if d.seller.email != "" [ · #d.seller.email ] #if d.seller.website != "" [ · #d.seller.website ]
  ]
]
#v(2pt)
#line(length: 100%, stroke: 1.4pt + ink)
#v(-9pt)
#line(length: 100%, stroke: 0.4pt + ink)

#v(7mm)
#align(center, text(15pt, fill: ink, tracking: 4pt, upper(d.doc.title)))
#v(7mm)

#grid(columns: (1fr, 1fr), column-gutter: 16pt,
  [
    #label[Billed to] \
    #v(2pt)
    #text(11.5pt, weight: "bold", d.client.name) \
    #if d.client.contact_person != "" and d.client.contact_person != d.client.name [ Attn. #d.client.contact_person \ ]
    #if d.client.address != "" [ #d.client.address \ ]
    #if d.client.tax_line != "" [ #d.client.tax_line \ ]
    #text(fill: muted)[#d.client.email #if d.client.phone != "" [ · #d.client.phone ]]
  ],
  [
    #label[Invoice details]
    #v(2pt)
    #table(columns: (auto, 1fr), stroke: none, inset: (x: 0pt, y: 2.5pt), column-gutter: 10pt,
      text(fill: muted)[Number], [*#d.doc.number*],
      text(fill: muted)[Date], d.doc.date,
      text(fill: muted)[Reference], d.record.title,
      text(fill: muted)[Prepared by], d.record.responsible,
    )
  ],
)

#v(8mm)
#let al = (c, r) => if c >= 2 { right } else { left }
#if d.has_products {
  table(
    columns: (auto, 1fr, auto, auto, auto, auto, auto),
    stroke: 0.4pt + rule,
    fill: (_, y) => if y == 0 { ink.lighten(88%) },
    inset: (x: 6pt, y: 5.5pt),
    align: al,
    table.header(..("No.", "Description", "Qty", "Unit price", "Disc.", "Tax", "Amount").map(h => text(8.5pt, weight: "bold", fill: ink, h))),
    ..d.products.map(p => (str(p.n), p.name, p.quantity + " " + p.unit, p.price.text, p.discount, p.tax_rate, p.total.text)).flatten(),
  )
} else {
  table(columns: (1fr, auto), stroke: 0.4pt + rule, inset: 6pt, align: (left, right), d.record.title, d.totals.total.text)
}

#v(3mm)
#align(right, block(width: 80mm, table(
  columns: (1fr, auto), stroke: none, inset: (x: 0pt, y: 3pt), align: (left, right),
  [Subtotal], d.totals.subtotal.text,
  ..if d.totals.has_discount { ([Discount], "−" + d.totals.discount.text) } else { () },
  ..d.taxes.map(t => ([Tax #t.rate on #t.base.text], t.tax.text)).flatten(),
  table.hline(stroke: 1.4pt + ink),
  text(12pt, weight: "bold", fill: ink)[Total due], text(12pt, weight: "bold", fill: ink, d.totals.total.text),
  table.hline(stroke: 0.4pt + ink),
)))

#v(10mm)
#grid(columns: (1fr, 62mm), column-gutter: 16pt,
  [
    #if d.seller.bank_details != "" [
      #label[Payment details] \
      #v(2pt)
      #d.seller.bank_details
      #v(4mm)
    ]
    #if d.record.comments != "" [
      #label[Terms and notes] \
      #v(2pt)
      #d.record.comments
    ]
  ],
  align(bottom)[
    #v(16mm)
    #line(length: 100%, stroke: 0.5pt + muted)
    #align(center, text(8.5pt, fill: muted)[Authorised signatory · #d.seller.name])
  ],
)
