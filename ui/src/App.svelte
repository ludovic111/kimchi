<script lang="ts">
  import { onMount } from "svelte";
  import { api, events, isTauri, message } from "$lib/ipc";
  import { editor } from "$lib/state/editor.svelte";
  import { gen } from "$lib/state/gen.svelte";
  import { ui } from "$lib/state/ui.svelte";
  import Home from "$lib/components/home/Home.svelte";
  import Editor from "$lib/components/editor/Editor.svelte";
  import Toasts from "$lib/components/ui/Toasts.svelte";
  import ContextMenu from "$lib/components/ui/ContextMenu.svelte";
  import SettingsDialog from "$lib/components/dialogs/SettingsDialog.svelte";

  let booted = $state(false);

  onMount(() => {
    const offs = [
      events.projectChanged((v) => {
        if (editor.project?.id === v.project.id) editor.set(v);
      }),
      events.job((j) => gen.upsertJob(j)),
      events.toast((m) => ui.toast(m)),
    ];
    (async () => {
      try {
        await gen.load();
        editor.set(await api.currentProject());
        if (!isTauri) await (await import("$lib/mock")).applyDemoParams();
      } catch (e) {
        ui.error(message(e));
      }
      booted = true;
    })();
    return () => offs.forEach((p) => p.then((off) => off()));
  });
</script>

{#if booted}
  {#if editor.project}
    <Editor />
  {:else}
    <Home />
  {/if}
{/if}

<SettingsDialog />
<ContextMenu />
<Toasts />
