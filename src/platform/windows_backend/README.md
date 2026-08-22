# Windows capture backend

These modules are derived from the MIT-licensed `flexaudio-os-windows` 0.2.0
backend. Vapor Tap keeps them in-tree to expose a shared termination flag when
the native WASAPI worker exits after a successful start.
