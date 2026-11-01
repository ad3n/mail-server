// SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
import { useState, type FormEvent } from 'react';
import { useSearchParams } from 'react-router-dom';
import { useQuery } from '@tanstack/react-query';
import { ArrowLeft, MoreHorizontal, Plus } from 'lucide-react';
import { apiFetch, apiFetchJson } from '@/services/api';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select';
import { CopyButton } from '@/components/common/CopyButton';
import { ConfirmDialog } from '@/components/common/ConfirmDialog';
import { FormSectionCard } from '@/components/forms/FormSections';
import { ListTable } from '@/components/lists/ListTable';
import { ListError, ListSpinner } from '@/components/lists/ListStates';
import { Dialog, DialogClose, DialogContent, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu';
import { useAuthStore } from '@/stores/authStore';
import { useSchemaStore } from '@/stores/schemaStore';

const authLabels = {
  none: 'None',
  basic: 'Basic',
  bearer: 'Bearer',
  apiKey: 'API key',
  apiKeyToken: 'API key + token',
} as const;
type AuthType = keyof typeof authLabels;
type WebhookAuth =
  | { type: 'none' | 'bearer' }
  | { type: 'basic'; username: string }
  | { type: 'apiKey'; keyHeader: string }
  | { type: 'apiKeyToken'; keyHeader: string; tokenHeader: string };
interface AppInput {
  name: string;
  webhookUrl: string;
  webhookAuth: WebhookAuth;
  webhookSecrets: Record<string, string>;
}
interface App {
  appId: string;
  name: string;
  webhookUrl: string;
  webhookAuth?: WebhookAuth;
}
interface Credentials {
  appKey: string;
  appToken: string;
}
const endpoint = '/api/integrations/apps';
const columns = [
  { name: 'name', label: 'Name' },
  { name: 'webhookUrl', label: 'Webhook URL' },
  { name: 'authType', label: 'Auth type' },
];

function AppForm({
  app,
  busy,
  cancel,
  save,
}: {
  app?: App;
  busy: boolean;
  cancel: () => void;
  save: (input: AppInput) => void;
}) {
  const previous = app?.webhookAuth;
  const [authType, setAuthType] = useState<AuthType>(previous?.type ?? 'none');
  const [url, setUrl] = useState(app?.webhookUrl ?? '');
  const keepSecrets = previous?.type === authType && app?.webhookUrl === url.trim();
  function authField(label: string, name: string, value = '', secret = false) {
    return (
      <label className="grid items-center gap-2 py-4 sm:grid-cols-3">
        <span className="text-sm font-semibold">{label}</span>
        <Input
          name={name}
          type={secret ? 'password' : 'text'}
          autoComplete={secret ? 'new-password' : 'off'}
          className="sm:col-span-2"
          defaultValue={value}
          required={!secret || !keepSecrets}
          placeholder={secret && keepSecrets ? 'Leave blank to keep the saved value' : undefined}
        />
      </label>
    );
  }
  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const data = new FormData(event.currentTarget);
    const text = (name: string) => String(data.get(name) ?? '');
    let webhookAuth: WebhookAuth;
    switch (authType) {
      case 'basic':
        webhookAuth = { type: authType, username: text('username') };
        break;
      case 'apiKey':
        webhookAuth = { type: authType, keyHeader: text('keyHeader') };
        break;
      case 'apiKeyToken':
        webhookAuth = { type: authType, keyHeader: text('keyHeader'), tokenHeader: text('tokenHeader') };
        break;
      default:
        webhookAuth = { type: authType };
    }
    const webhookSecrets: Record<string, string> = {};
    for (const field of ['password', 'apiKey', 'token']) if (text(field)) webhookSecrets[field] = text(field);
    save({ name: text('name').trim(), webhookUrl: url.trim(), webhookAuth, webhookSecrets });
  }
  return (
    <form onSubmit={submit} className="space-y-3.5">
      <header className="flex items-start gap-3.5 px-1 pb-1 pt-3">
        <Button type="button" variant="outline" size="icon-lg" aria-label="Back" onClick={cancel} disabled={busy}>
          <ArrowLeft />
        </Button>
        <h1 className="page-title pt-1">{app ? `Edit app: ${app.name}` : 'Create app'}</h1>
      </header>
      <FormSectionCard>
        <label className="grid items-center gap-2 py-4 sm:grid-cols-3">
          <span className="text-sm font-semibold">Name</span>
          <Input name="name" className="sm:col-span-2" required maxLength={128} defaultValue={app?.name} />
        </label>
        <label className="grid items-center gap-2 py-4 sm:grid-cols-3">
          <span className="text-sm font-semibold">Webhook URL</span>
          <Input
            name="url"
            className="sm:col-span-2"
            type="url"
            required
            value={url}
            onChange={(e) => setUrl(e.target.value)}
            placeholder="https://example.com/webhook"
          />
        </label>
        <div className="grid items-center gap-2 py-4 sm:grid-cols-3">
          <label htmlFor="webhook-auth-type" className="text-sm font-semibold">
            Auth type
          </label>
          <Select value={authType} onValueChange={(val) => setAuthType(val as AuthType)}>
            <SelectTrigger id="webhook-auth-type" className="sm:col-span-2 w-full">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {(Object.keys(authLabels) as AuthType[]).map((t) => (
                <SelectItem value={t} key={t}>
                  {authLabels[t]}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
        {authType !== 'none' && (
          <div key={authType} className="divide-y divide-border">
            {authType === 'basic' && (
              <>
                {authField('Username', 'username', previous?.type === 'basic' ? previous.username : '')}
                {authField('Password', 'password', '', true)}
              </>
            )}
            {authType === 'bearer' && authField('Bearer token', 'token', '', true)}
            {(authType === 'apiKey' || authType === 'apiKeyToken') && (
              <>
                {authField(
                  'API key header name',
                  'keyHeader',
                  previous && 'keyHeader' in previous ? previous.keyHeader : 'X-API-Key',
                )}
                {authField('API key value', 'apiKey', '', true)}
              </>
            )}
            {authType === 'apiKeyToken' && (
              <>
                {authField(
                  'Token header name',
                  'tokenHeader',
                  previous?.type === 'apiKeyToken' ? previous.tokenHeader : 'X-API-Token',
                )}
                {authField('Token value', 'token', '', true)}
              </>
            )}
          </div>
        )}
      </FormSectionCard>
      <div className="flex justify-end gap-2">
        <Button type="button" variant="outline" disabled={busy} onClick={cancel}>
          Cancel
        </Button>
        <Button disabled={busy}>Save</Button>
      </div>
    </form>
  );
}

export function IntegrationApps() {
  const account = useAuthStore((s) => s.activeAccountId);
  const schema = useSchemaStore((s) => s.schema);
  const apps = useQuery({
    queryKey: ['integration-apps', account],
    queryFn: ({ signal }) => apiFetchJson<App[]>(endpoint, { signal }),
  });
  const [params, setParams] = useSearchParams();
  const editing = params.get('edit');
  const app = apps.data?.find((a) => a.appId === editing);
  const [credentials, setCredentials] = useState<({ name: string } & Credentials) | null>(null);
  const [confirm, setConfirm] = useState<{ action: 'Delete' | 'Regenerate'; app: App } | null>(null);
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);

  async function run(fn: () => Promise<void>) {
    setBusy(true);
    setError('');
    try {
      await fn();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }
  function list() {
    setParams({});
    setError('');
  }
  async function save(input: AppInput) {
    await run(async () => {
      const res = await apiFetch(app ? endpoint + '/' + app.appId : endpoint, {
        method: app ? 'PUT' : 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(input),
      });
      if (!app)
        setCredentials({
          name: input.name,
          appKey: res.headers.get('App-Key')!,
          appToken: res.headers.get('App-Token')!,
        });
      list();
      await apps.refetch();
    });
  }
  async function show(target: App, rotate = false) {
    const path = endpoint + '/' + target.appId + '/credentials';
    const values = rotate
      ? await apiFetchJson<Credentials>(path + '/regenerate', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: '{}',
        })
      : await apiFetchJson<Credentials>(path);
    setCredentials({ name: target.name, ...values });
  }

  if (!schema || apps.isPending) return <ListSpinner />;
  if (apps.error) return <ListError message={apps.error.message} onRetry={() => void apps.refetch()} />;
  const items = apps.data.map((a) => ({ ...a, id: a.appId, authType: authLabels[a.webhookAuth?.type ?? 'none'] }));
  return (
    <div className="space-y-3.5">
      {error && <ListError message={error} onDismiss={() => setError('')} />}
      {editing && !app ? (
        <ListError message="App not found." onDismiss={list} />
      ) : app || params.has('create') ? (
        <AppForm key={app?.appId ?? 'new'} app={app} busy={busy} cancel={list} save={save} />
      ) : (
        <>
          <header className="flex flex-wrap items-end justify-between gap-4 px-1 pb-1 pt-3">
            <div>
              <h1 className="page-title">Apps</h1>
              <p className="mt-1 text-sm text-muted-foreground">
                Manage delivery webhooks and application credentials.
              </p>
            </div>
            <Button onClick={() => setParams({ create: '1' })}>
              <Plus aria-hidden />
              Create app
            </Button>
          </header>
          <section className="tile relative overflow-hidden">
            <ListTable
              caption="Apps"
              columns={columns}
              fields={{}}
              items={items}
              schema={schema}
              objectName="IntegrationApps"
              sort={null}
              sortable={new Set()}
              onSort={() => {}}
              selection={null}
              rowHref={(id) => '?edit=' + encodeURIComponent(id)}
              rowLabel={(item) => String(item.name)}
              loading={false}
              busy={busy || apps.isFetching}
              renderRowActions={(_, id, name) => {
                const target = apps.data.find((a) => a.appId === id)!;
                return (
                  <DropdownMenu>
                    <DropdownMenuTrigger asChild>
                      <Button variant="ghost" size="icon-sm" disabled={busy} aria-label={'Actions for ' + name}>
                        <MoreHorizontal aria-hidden />
                      </Button>
                    </DropdownMenuTrigger>
                    <DropdownMenuContent align="end">
                      <DropdownMenuItem onSelect={() => setParams({ edit: id })}>Edit</DropdownMenuItem>
                      <DropdownMenuItem onSelect={() => void run(() => show(target))}>
                        View credentials
                      </DropdownMenuItem>
                      <DropdownMenuItem onSelect={() => setConfirm({ action: 'Regenerate', app: target })}>
                        Regenerate credentials
                      </DropdownMenuItem>
                      <DropdownMenuSeparator />
                      <DropdownMenuItem
                        className="text-destructive focus:bg-destructive-soft focus:text-destructive"
                        onSelect={() => setConfirm({ action: 'Delete', app: target })}
                      >
                        Delete
                      </DropdownMenuItem>
                    </DropdownMenuContent>
                  </DropdownMenu>
                );
              }}
            />
          </section>
        </>
      )}
      <ConfirmDialog
        open={!!confirm}
        onOpenChange={(open) => {
          if (!open) setConfirm(null);
        }}
        title={confirm?.action === 'Delete' ? 'Delete app?' : 'Regenerate credentials?'}
        confirmLabel={confirm?.action}
        variant="destructive"
        description={
          confirm?.action === 'Delete'
            ? 'Delete ' + confirm.app.name + ' and revoke its credentials?'
            : 'The current App-Key and App-Token for ' + (confirm?.app.name ?? '') + ' will be revoked immediately.'
        }
        onConfirm={() =>
          run(async () => {
            if (!confirm) return;
            if (confirm.action === 'Regenerate') await show(confirm.app, true);
            else {
              await apiFetch(endpoint + '/' + confirm.app.appId, { method: 'DELETE' });
              await apps.refetch();
            }
          })
        }
      />
      <Dialog
        open={!!credentials}
        onOpenChange={(open) => {
          if (!open) setCredentials(null);
        }}
      >
        <DialogContent aria-describedby={undefined}>
          <DialogHeader>
            <DialogTitle>App credentials: {credentials?.name}</DialogTitle>
          </DialogHeader>
          {credentials && (
            <div className="divide-y divide-border rounded-xl bg-muted px-4 py-1">
              {(
                [
                  ['App-Key', credentials.appKey],
                  ['App-Token', credentials.appToken],
                ] as const
              ).map(([label, value]) => (
                <div key={label} className="grid items-center gap-2 py-4 sm:grid-cols-3">
                  <label htmlFor={label} className="text-sm font-semibold">
                    {label}
                  </label>
                  <div className="flex min-w-0 items-center gap-2 sm:col-span-2">
                    <Input
                      id={label}
                      className="min-w-0 flex-1 font-mono"
                      readOnly
                      type={label === 'App-Token' ? 'password' : 'text'}
                      value={value}
                      onFocus={(e) => e.target.select()}
                    />
                    <CopyButton value={value} label={label} />
                  </div>
                </div>
              ))}
            </div>
          )}
          <DialogFooter>
            <DialogClose asChild>
              <Button>Done</Button>
            </DialogClose>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}
