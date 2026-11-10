// NERP example invoice "Modern" (Typst): colour band header, summary cards, striped rows.
// Data: see the "Fields" list in CRM settings → Documents.
#let d = json("data.json")
#let accent = rgb("#00695c")
#let soft = rgb("#e0f2f1")
#let muted = luma(105)

#set document(title: d.doc.title + " " + d.doc.number)
#set page(
  paper: "a4",
  margin: (x: 16mm, top: 14mm, bottom: 20mm),
  // The band sits behind the first page's header block (14 mm margin + 36 mm block).
  background: context if counter(page).get().first() == 1 { place(top, rect(width: 100%, height: 50mm, fill: accent)) },
  footer: context [
    #set text(7.5pt, fill: muted)
    #line(length: 100%, stroke: 0.4pt + luma(220))
    #v(-4pt)
    #d.seller.legal_name
    #if d.seller.tax_id != "" [ · Tax ID #d.seller.tax_id ]
    #if d.seller.registration_no != "" [ · #d.seller.registration_no ]
    #h(1fr) #d.doc.number · #counter(page).display("1 / 1", both: true)
  ],
)
#set text(
  font: ("Noto Sans", "Noto Sans Devanagari", "Noto Sans Bengali", "Noto Sans Arabic", "Noto Sans Hebrew",
         "Noto Sans CJK SC", "Noto Sans CJK JP", "Noto Sans Thai", "Noto Sans Armenian", "Noto Sans Georgian"),
  size: 9.5pt,
  lang: d.doc.locale.split("-").at(0),
)
#set par(leading: 0.55em)
#let label(t) = text(7.5pt, weight: "bold", fill: muted, tracking: 0.6pt, upper(t))

#block(height: 36mm, width: 100%, grid(columns: (1fr, auto), gutter: 12pt,
  [
    #set text(fill: white)
    #if d.seller.has_logo {
      box(fill: white, inset: 5pt, radius: 4pt, image("logo", height: 12mm))
    } else {
      text(18pt, weight: "bold", d.seller.name)
    }
    #v(4pt)
    #text(8.5pt, fill: white.darken(12%))[
      #d.seller.address \
      #d.seller.phone #if d.seller.email != "" [ · #d.seller.email ] #if d.seller.website != "" [ · #d.seller.website ]
    ]
  ],
  align(right)[
    #set text(fill: white)
    #stack(dir: ttb, spacing: 10pt, text(30pt, weight: "bold", upper(d.doc.title)), text(11pt, d.doc.number))
  ],
))

#v(8mm)
#let card(title, body, strong: false) = rect(width: 100%, fill: if strong { soft } else { luma(246) }, radius: 6pt, inset: 10pt)[
  #label(title) \
  #v(1pt)
  #text(if strong { 15pt } else { 11pt }, weight: "bold", fill: if strong { accent } else { black }, body)
]
#grid(columns: (1fr, 1fr, 1.3fr), gutter: 10pt,
  card("Issued", d.doc.date),
  card("Reference", d.record.title),
  card("Amount due", d.totals.total.text, strong: true),
)

#v(7mm)
#grid(columns: (1fr, 1fr), gutter: 12pt,
  [
    #label[Billed to] \
    #v(1pt)
    #text(11pt, weight: "bold", d.client.name) \
    #if d.client.contact_person != "" and d.client.contact_person != d.client.name [ #d.client.contact_person \ ]
    #if d.client.address != "" [ #d.client.address \ ]
    #if d.client.tax_line != "" [ #d.client.tax_line \ ]
    #text(fill: muted)[#d.client.email #if d.client.phone != "" [ · #d.client.phone ]]
  ],
  [
    #label[Contact] \
    #v(1pt)
    #d.record.responsible \
    #text(fill: muted)[#d.seller.email]
  ],
)

#v(7mm)
#let al = (c, r) => if c >= 2 { right } else { left }
#if d.has_products {
  table(
    columns: (auto, 1fr, auto, auto, auto, auto, auto),
    stroke: none,
    fill: (_, y) => if y == 0 { accent } else if calc.even(y) { luma(247) },
    inset: (x: 7pt, y: 6.5pt),
    align: al,
    table.header(..("#", "Item", "Qty", "Price", "Discount", "Tax", "Amount").map(h => text(8pt, weight: "bold", fill: white, h))),
    ..d.products.map(p => (str(p.n), p.name, p.quantity + " " + p.unit, p.price.text, p.discount, p.tax_rate, text(weight: "bold", p.total.text))).flatten(),
  )
} else {
  table(columns: (1fr, auto), stroke: none, fill: luma(247), inset: 8pt, align: (left, right), d.record.title, d.totals.total.text)
}

#v(4mm)
#align(right, block(width: 82mm)[
  #table(
    columns: (1fr, auto), stroke: none, inset: (x: 7pt, y: 3pt), align: (left, right),
    text(fill: muted)[Subtotal], d.totals.subtotal.text,
    ..if d.totals.has_discount { (text(fill: muted)[Discount], "−" + d.totals.discount.text) } else { () },
    ..d.taxes.map(t => (text(fill: muted)[Tax #t.rate on #t.base.text], t.tax.text)).flatten(),
  )
  #v(-2pt)
  #rect(width: 100%, fill: accent, radius: 6pt, inset: (x: 10pt, y: 9pt))[
    #set text(fill: white, weight: "bold")
    #text(10pt)[Total] #h(1fr) #text(14pt, d.totals.total.text)
  ]
])

#v(8mm)
#if d.seller.bank_details != "" {
  rect(width: 100%, stroke: (left: 3pt + accent), inset: (left: 10pt, y: 6pt))[
    #label[How to pay] \
    #v(1pt)
    #d.seller.bank_details
  ]
}
#if d.record.comments != "" [
  #v(4mm)
  #label[Notes] \
  #v(1pt)
  #d.record.comments
]
#v(6mm)
#align(center, text(10pt, weight: "bold", fill: accent)[Thank you for your business!])
