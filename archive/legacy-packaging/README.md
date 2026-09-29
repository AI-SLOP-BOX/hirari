# Archived local application bundle

The pre-existing `packaging/Hirari.app` was an ignored 63 MiB generated
bundle. Its executable and worker still used Aura names, so it was not a
current Hirari build. It has been moved here to keep a local rollback copy out
of the active packaging directory. Generated `.app` bundles remain excluded
from version control; the canonical build output is `packaging/Hirari DAW.app`.
