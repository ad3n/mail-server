# Integration apps in the Stalwart admin WebUI

The server serves the admin WebUI from an Application bundle. This overlay adds Management → Directory → Apps to the WebUI and uses its authenticated API client to manage delivery integrations.

To build the bundle, run these commands from the mail-server checkout. Use a fresh WebUI checkout at the tested revision.

```sh
overlay_dir="$PWD/resources/webui"
git clone https://github.com/stalwartlabs/webui.git webui-build
cd webui-build
git checkout 4414cd585b0cae9d271487ff5619631d9c752b04
git apply "$overlay_dir/integrations.patch"
mkdir -p src/features/integrations
cp "$overlay_dir/IntegrationApps.tsx" src/features/integrations/
bash build.sh
```

Set the existing Stalwart Web Interface application's resource URL to the generated `.ignore/webui.zip`, using an absolute `file://` URL accessible to the server or an HTTPS URL. Keep the existing URL prefixes and run the **Update Apps** action to load the bundle. An already open admin tab needs to be reloaded.
