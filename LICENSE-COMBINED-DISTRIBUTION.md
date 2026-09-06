# Combined distribution licensing

This file applies only when Aura is distributed together with third-party
software or plugin binaries. It does not change the MIT license of Aura-owned
source code in this repository.

Optional Aura frontends or tools may be marked GPL-3.0-only and distributed as
separate components. Combining one of those components into a single product
requires following GPL-3.0 for that combined work; it does not relicense the
independent Aura engine or stable API, which remain MIT.

Third-party components retain their own copyright and license terms. A build
that links or bundles GPL-licensed components may impose additional
obligations on that combined distribution. Such a build must ship the
applicable source, notices, and license texts required by those components.

The desktop bundle selects Slint's royalty-free license and includes the
`AboutSlint` attribution widget in the Help/Diagnostics surface. That choice
permits Slint to be distributed as part of the Aura application with the
required attribution; it does not permit standalone redistribution of Slint.
The royalty-free terms also do not permit distributing an Application that
exposes Slint's APIs, in part or in total. Aura's public stable API is an Aura
contract and must not forward or re-export Slint types, symbols, or handles.
Distributors may choose Slint GPL-3.0 or a commercial Slint license instead,
provided they satisfy that license's terms.

Installed plugins such as Surge XT and Vital are not included in the Aura
source publication. Users obtain and license those products separately.

OpenUtau is also treated as a separately licensed upstream project; Aura ships
the bridge and integration contract, not the local review clone or installed
application.
