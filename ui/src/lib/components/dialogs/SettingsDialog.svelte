<script lang="ts">
  import { Check, CircleAlert, Cloud, Cpu, ExternalLink, KeyRound, LoaderCircle, PlugZap } from "@lucide/svelte";
  import { openUrl } from "@tauri-apps/plugin-opener";
  import { api, message } from "$lib/ipc";
  import type { ProviderStatus } from "$lib/bindings/ProviderStatus";
  import { gen } from "$lib/state/gen.svelte";
  import { ui } from "$lib/state/ui.svelte";
  import Button from "../ui/Button.svelte";
  import Dialog from "../ui/Dialog.svelte";
  import Toggle from "../ui/Toggle.svelte";

  /** Provider-specific options worth a field. Keys match each provider's docs. */
  const OPTIONS: Record<string, { key: string; label: string; placeholder: string; help: string }[]> = {
    comfyui: [{ key: "workflows_dir", label: "Workflows folder", placeholder: "~/Documents/kimchi/comfyui-workflows", help: "API-format workflow JSON files with {{prompt}}, {{image}}, {{seed}}… placeholders become models." }],
    fal: [{ key: "models", label: "Extra models", placeholder: "fal-ai/some-model, fal-ai/another", help: "Comma-separated endpoint ids to add to the picker." }],
    replicate: [{ key: "models", label: "Extra models", placeholder: "owner/model, owner/model:version", help: "Comma-separated. Inputs are read from each model's schema." }],
    openai_compat: [{ key: "model", label: "Model id", placeholder: "e.g. stablediffusion", help: "Used when the server doesn't list its models." }],
  };

  let selectedId = $state<string | null>(null);
  let keyDraft = $state("");
  let baseDraft = $state("");
  let checking = $state(false);
  let check = $state<{ ok: boolean; text: string } | null>(null);

  const providers = $derived(gen.providers);
  const selected = $derived(providers.find((p) => p.info.id === selectedId) ?? providers[0]);
  const cloud = $derived(providers.filter((p) => p.info.kind === "cloud"));
  const local = $derived(providers.filter((p) => p.info.kind === "local"));

  $effect(() => {
    if (ui.settingsOpen) {
      selectedId = ui.settingsProvider ?? selectedId ?? providers.find((p) => p.ready)?.info.id ?? "openrouter";
    }
  });

  $effect(() => {
    void selected?.info.id;
    keyDraft = "";
    baseDraft = selected?.settings.base_url ?? "";
    check = null;
  });

  async function saveKey() {
    if (!selected || !keyDraft.trim()) return;
    try {
      gen.setProviders(await api.genSetKey(selected.info.id, keyDraft.trim()));
      keyDraft = "";
      await test();
    } catch (e) {
      ui.error(message(e));
    }
  }

  async function removeKey() {
    if (!selected) return;
    gen.setProviders(await api.genSetKey(selected.info.id, null));
    check = null;
  }

  async function saveSettings(patch: Partial<ProviderStatus["settings"]>) {
    if (!selected) return;
    const next = { ...selected.settings, ...patch };
    gen.setProviders(await api.genSetSettings(selected.info.id, next));
  }

  async function test() {
    if (!selected) return;
    checking = true;
    check = null;
    try {
      check = { ok: true, text: await api.genCheck(selected.info.id) };
    } catch (e) {
      check = { ok: false, text: message(e) };
    } finally {
      checking = false;
    }
  }

  const status = (p: ProviderStatus) => (p.ready ? "ready" : !p.settings.enabled ? "off" : p.info.needs_key ? "nokey" : "ready");
</script>

<Dialog open={ui.settingsOpen} onclose={() => (ui.settingsOpen = false)} title="Models & keys" subtitle="Keys are stored in your system keychain and only sent to the provider they belong to." width={860}>
  <div class="layout">
    <nav>
      <span class="eyebrow"><Cloud size={11} /> Cloud</span>
      {#each cloud as p (p.info.id)}
        <button class="item" class:on={selected?.info.id === p.info.id} onclick={() => (selectedId = p.info.id)}>
          <span class="dot {status(p)}"></span>{p.info.name}
        </button>
      {/each}
      <span class="eyebrow"><Cpu size={11} /> On your machine</span>
      {#each local as p (p.info.id)}
        <button class="item" class:on={selected?.info.id === p.info.id} onclick={() => (selectedId = p.info.id)}>
          <span class="dot {status(p)}"></span>{p.info.name}
        </button>
      {/each}
    </nav>

    {#if selected}
      {@const p = selected}
      <div class="detail">
        <div class="phead">
          <div>
            <h3>{p.info.name}</h3>
            <p>{p.info.tagline}</p>
          </div>
          <Toggle label={p.settings.enabled ? "Enabled" : "Disabled"} checked={p.settings.enabled} onchange={(v) => saveSettings({ enabled: v })} />
        </div>

        <div class="tasks">
          {#each p.info.tasks as t (t)}<span>{t.replaceAll("_", " ").replace(/^./, (c) => c.toUpperCase())}</span>{/each}
        </div>

        {#if p.info.needs_key || p.info.key_hint}
          <div class="field">
            <div class="flabel">
              <span>API key</span>
              {#if p.info.key_url}
                <button class="link" onclick={() => openUrl(p.info.key_url!)}>Get a key <ExternalLink size={11} /></button>
              {/if}
            </div>
            {#if p.key_preview}
              <div class="saved">
                <KeyRound size={13} />
                <span class="mono">{p.key_preview}</span>
                <span class="src">{p.key_source === "env" ? `from ${p.info.key_env.join(" / ")}` : "in keychain"}</span>
                {#if p.key_source === "keychain"}<button class="link danger" onclick={removeKey}>Remove</button>{/if}
              </div>
            {/if}
            <form
              class="keyrow"
              onsubmit={(e) => {
                e.preventDefault();
                saveKey();
              }}
            >
              <input type="password" autocomplete="off" spellcheck="false" placeholder={p.key_preview ? "Replace key" : (p.info.key_hint ?? "Paste your key")} bind:value={keyDraft} onkeydown={(e) => e.stopPropagation()} />
              <Button type="submit" variant="primary" disabled={!keyDraft.trim()}>Save</Button>
            </form>
            {#if p.info.key_env.length}<small>Or set <code>{p.info.key_env[0]}</code> in your environment.</small>{/if}
          </div>
        {/if}

        {#if p.info.base_url_editable}
          <div class="field">
            <div class="flabel"><span>Server address</span></div>
            <input placeholder={p.info.default_base_url} bind:value={baseDraft} onblur={() => saveSettings({ base_url: baseDraft.trim() || null })} onkeydown={(e) => e.stopPropagation()} />
          </div>
        {/if}

        {#each OPTIONS[p.info.id] ?? [] as o (o.key)}
          <div class="field">
            <div class="flabel"><span>{o.label}</span></div>
            <input
              placeholder={o.placeholder}
              value={String(p.settings.options[o.key] ?? "")}
              onblur={(e) => saveSettings({ options: { ...p.settings.options, [o.key]: (e.currentTarget as HTMLInputElement).value.trim() || undefined } })}
              onkeydown={(e) => e.stopPropagation()}
            />
            <small>{o.help}</small>
          </div>
        {/each}

        <div class="test">
          <Button onclick={test} disabled={checking}>
            {#if checking}<LoaderCircle size={13} class="spin" />{:else}<PlugZap size={13} />{/if}
            Test connection
          </Button>
          {#if check}
            <span class="result" class:ok={check.ok}>
              {#if check.ok}<Check size={13} />{:else}<CircleAlert size={13} />{/if}
              {check.text}
            </span>
          {/if}
        </div>

        <button class="site" onclick={() => openUrl(p.info.website)}>{p.info.website.replace(/^https?:\/\//, "")} <ExternalLink size={11} /></button>
      </div>
    {/if}
  </div>
</Dialog>

<style>
  .layout {
    display: grid;
    grid-template-columns: 210px 1fr;
    gap: 22px;
    min-height: 440px;
  }
  nav {
    display: flex;
    flex-direction: column;
    gap: 1px;
    max-height: 60vh;
    overflow: auto;
    padding-right: 6px;
    border-right: 1px solid var(--line);
  }
  nav .eyebrow {
    display: flex;
    align-items: center;
    gap: 5px;
    padding: 10px 8px 6px;
  }
  nav .eyebrow:first-child {
    padding-top: 0;
  }
  .item {
    display: flex;
    align-items: center;
    gap: 9px;
    height: 30px;
    padding: 0 9px;
    border-radius: var(--r-sm);
    font-size: 12.5px;
    color: var(--text-2);
    text-align: left;
  }
  .item:hover {
    background: var(--hover);
    color: var(--text);
  }
  .item.on {
    background: var(--surface-3);
    color: var(--text);
  }
  .dot {
    width: 7px;
    height: 7px;
    border-radius: 99px;
    background: var(--surface-4);
    box-shadow: inset 0 0 0 1px var(--line-3);
    flex: none;
  }
  .dot.ready {
    background: var(--green);
    box-shadow: 0 0 8px rgba(166, 207, 94, 0.6);
  }
  .dot.off {
    background: transparent;
  }
  .detail {
    display: flex;
    flex-direction: column;
    gap: 18px;
    min-width: 0;
  }
  .phead {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    gap: 16px;
  }
  h3 {
    margin: 0;
    font-size: 20px;
    font-weight: 600;
    letter-spacing: -0.01em;
  }
  .phead p {
    margin: 3px 0 0;
    color: var(--text-2);
  }
  .tasks {
    display: flex;
    gap: 5px;
    flex-wrap: wrap;
    margin-top: -6px;
  }
  .tasks span {
    font-size: 11px;
    padding: 2px 8px;
    border-radius: 99px;
    background: var(--surface-3);
    color: var(--text-2);
  }
  .field {
    display: flex;
    flex-direction: column;
    gap: 7px;
  }
  .flabel {
    display: flex;
    justify-content: space-between;
    font-size: 12px;
    font-weight: 600;
    color: var(--text-2);
  }
  .link {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    color: var(--accent-hi);
    font-size: 12px;
    font-weight: 550;
  }
  .link:hover {
    text-decoration: underline;
  }
  .link.danger {
    color: var(--red);
    margin-left: auto;
  }
  .saved {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 10px;
    border-radius: var(--r-sm);
    background: var(--green-soft);
    color: var(--green);
    font-size: 12px;
  }
  .src {
    color: var(--text-3);
  }
  .keyrow {
    display: flex;
    gap: 8px;
  }
  input {
    flex: 1;
    height: 34px;
    padding: 0 11px;
    border-radius: var(--r-sm);
    background: var(--bg-sunken);
    border: 0;
    outline: 0;
    box-shadow: inset 0 0 0 1px var(--line-2);
    font-size: 13px;
    font-family: var(--font-mono);
  }
  input:focus {
    box-shadow:
      inset 0 0 0 1px var(--accent-line),
      0 0 0 3px rgba(255, 90, 54, 0.1);
  }
  small {
    color: var(--text-3);
    font-size: 11.5px;
  }
  code {
    font-family: var(--font-mono);
    color: var(--text-2);
  }
  .test {
    display: flex;
    align-items: center;
    gap: 12px;
  }
  .test :global(.spin) {
    animation: spin 0.8s linear infinite;
  }
  .result {
    display: flex;
    align-items: flex-start;
    gap: 6px;
    font-size: 12px;
    color: var(--red);
    min-width: 0;
    overflow-wrap: anywhere;
  }
  .result :global(svg) {
    flex: none;
    margin-top: 1px;
  }
  .result.ok {
    color: var(--green);
  }
  .site {
    margin-top: auto;
    align-self: flex-start;
    display: inline-flex;
    align-items: center;
    gap: 5px;
    color: var(--text-3);
    font-size: 12px;
  }
  .site:hover {
    color: var(--text);
  }
</style>
