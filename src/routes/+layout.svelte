<script lang="ts">
	import "../app.css";
	import { onMount } from "svelte";
	import { initTheme } from "$lib/theme";
	import type { UnlistenFn } from "$lib/api";

	let { children } = $props();

	onMount(() => {
		let unlisten: UnlistenFn | undefined;
		// initTheme applies the kanagawa fallback synchronously, then upgrades.
		initTheme()
			.then((u) => (unlisten = u))
			.catch(() => {});
		return () => unlisten?.();
	});
</script>

{@render children()}
