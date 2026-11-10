// NERP built-in quote (Typst). Data: see the "Fields" list in CRM settings → Documents.
#let d = json("data.json")
#let accent = rgb("#0b57d0")
#let muted = luma(110)

#set document(title: d.doc.title + " " + d.doc.number)
#set page(paper: "a4", margin: (x: 18mm, top: 16mm, bottom: 20mm), footer: context [
  #set text(7.5pt, fill: muted)
  #d.seller.legal_name
  #if d.seller.tax_id != "" [ · Tax ID #d.seller.tax_id ]
  #if d.seller.registration_no != "" [ · #d.seller.registration_no ]
  #h(1fr) #counter(page).display("1 / 1", both: true)
])
// Font list = fallback chain, so any script in customer data renders.
#set text(
  font: ("Noto Sans", "Noto Sans Devanagari", "Noto Sans Bengali", "Noto Sans Arabic", "Noto Sans Hebrew",
         "Noto Sans CJK SC", "Noto Sans CJK JP", "Noto Sans Thai", "Noto Sans Armenian", "Noto Sans Georgian"),
  size: 9.5pt,
  lang: d.doc.locale.split("-").at(0),
)
#set par(leading: 0.55em)

#grid(columns: (1fr, auto), gutter: 12pt,
  [
    #if d.seller.has_logo { image("logo", height: 14mm) } else { text(15pt, weight: "bold", d.seller.name) }
    #v(2pt)
    #text(fill: muted)[
      #d.seller.address \
      #d.seller.phone #if d.seller.email != "" [ · #d.seller.email ] #if d.seller.website != "" [ · #d.seller.website ]
    ]
  ],
  align(right)[
    #text(22pt, weight: "bold", fill: accent, upper(d.doc.title)) \
    #v(2pt)
    #table(columns: 2, stroke: none, inset: (x: 0pt, y: 1.5pt), align: (right, right), column-gutter: 8pt,
      text(fill: muted)[No.], [*#d.doc.number*],
      text(fill: muted)[Date], d.doc.date,
    )
  ],
)

#v(10mm)
#grid(columns: (1fr, 1fr), gutter: 12pt,
  [
    #text(8pt, weight: "bold", fill: muted, tracking: 0.5pt)[PREPARED FOR] \
    #text(11pt, weight: "bold", d.client.name) \
    #if d.client.contact_person != "" and d.client.contact_person != d.client.name [ #d.client.contact_person \ ]
    #if d.client.address != "" [ #d.client.address \ ]
    #if d.client.tax_line != "" [ #d.client.tax_line \ ]
    #text(fill: muted)[#d.client.email #if d.client.phone != "" [ · #d.client.phone ]]
  ],
  align(right)[
    #text(8pt, weight: "bold", fill: muted, tracking: 0.5pt)[REFERENCE] \
    #d.record.title \
    #text(fill: muted)[#d.record.type \##d.record.number]
  ],
)

#v(8mm)
#let num = (c, r) => if c >= 2 { right } else { left }
#if d.has_products {
  table(
    columns: (auto, 1fr, auto, auto, auto, auto, auto),
    stroke: (x, y) => if y == 0 { (bottom: 0.8pt + accent) } else { (bottom: 0.3pt + luma(220)) },
    inset: (x: 5pt, y: 5pt),
    align: num,
    table.header(..("#", "Item", "Qty", "Price", "Discount", "Tax", "Amount").map(h => text(8pt, weight: "bold", fill: muted, h))),
    ..d.products.map(p => (str(p.n), p.name, p.quantity + " " + p.unit, p.price.text, p.discount, p.tax_rate, p.total.text)).flatten(),
  )
} else {
  table(columns: (1fr, auto), stroke: (bottom: 0.3pt + luma(220)), inset: 6pt, align: (left, right), d.record.title, d.totals.total.text)
}

#v(4mm)
#align(right, block(width: 75mm, table(
  columns: (1fr, auto), stroke: none, inset: (x: 0pt, y: 3pt), align: (left, right),
  text(fill: muted)[Subtotal], d.totals.subtotal.text,
  ..if d.totals.has_discount { (text(fill: muted)[Discount], "−" + d.totals.discount.text) } else { () },
  ..d.taxes.map(t => (text(fill: muted)[Tax #t.rate on #t.base.text], t.tax.text)).flatten(),
  table.hline(stroke: 0.8pt + accent),
  text(11pt, weight: "bold")[Total], text(11pt, weight: "bold", d.totals.total.text),
)))

#v(8mm)
#text(8pt, weight: "bold", fill: muted, tracking: 0.5pt)[VALIDITY] \
This quote is valid for 30 days from #d.doc.date. To accept, reply to #if d.seller.email != "" [#d.seller.email] else [us] quoting #d.doc.number.
#if d.record.comments != "" [
  #v(5mm)
  #text(8pt, weight: "bold", fill: muted, tracking: 0.5pt)[NOTES] \
  #d.record.comments
]
