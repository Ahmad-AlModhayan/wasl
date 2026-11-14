# NavigationBar

The four top-level destinations, with the Problems badge.

**Provide:** `active` (`pairs` · `problems` · `bin` · `settings`) and `badges` (`{ problems: 3 }`).

- The Problems badge counts approvals, conflicts and failures only. No other destination ever badges.
- Active: filled icon on the `secondary-container` indicator.

**Compose:** `NavigationBar { NavigationBarItem(icon = { BadgedBox(badge = { Badge { Text("3") } }) { Icon } }) }`.
