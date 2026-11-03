# Integration apps in the Stalwart admin WebUI

The server serves the admin WebUI from an Application bundle. The source files under `src/` add Management → Directory → Apps to the WebUI and use its authenticated API client to manage delivery integrations.

Run `task build` from the mail-server checkout to build the server and WebUI together. Use `task build:ui` for the WebUI alone or `task build:server` for the server alone. The UI build requires Git, npm, Bash, tar and zip. It uses the pinned upstream revision and the local source files under `resources/webui/src/`.

Set the existing Stalwart Web Interface application's resource URL to the generated `.stalwart-local/webui-integrations.zip`, using an absolute `file://` URL accessible to the server or an HTTPS URL. Keep the existing URL prefixes and run the **Update Apps** action to load the bundle. An already open admin tab needs to be reloaded.
