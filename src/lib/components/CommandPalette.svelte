<script lang="ts">
	import { onMount } from "svelte";
	import { rankCommands, type PaletteCommand } from "$lib/palette";

	let {
		commands,
		onClose,
		placeholder = "Run a command…",
	}: {
		commands: PaletteCommand[];
		onClose: () => void;
		placeholder?: string;
	} = $props();

	let query = $state("");
	let selectedIndex = $state(0);
	let inputEl = $state<HTMLInputElement>();
	let listEl = $state<HTMLElement>();

	const results = $derived(rankCommands(query, commands));

	// Reset the highlight to the first result whenever the query changes.
	$effect(() => {
		query;
		selectedIndex = 0;
	});

	onMount(() => inputEl?.focus());

	const LISTBOX_ID = "command-palette-listbox";
	function optionId(index: number): string {
		return `command-palette-option-${index}`;
	}

	function scrollSelectedIntoView() {
		queueMicrotask(() => {
			listEl?.querySelector<HTMLElement>(`#${optionId(selectedIndex)}`)?.scrollIntoView({
				block: "nearest",
			});
		});
	}

	function move(delta: number) {
		const n = results.length;
		if (n === 0) return;
		selectedIndex = (selectedIndex + delta + n) % n;
		scrollSelectedIntoView();
	}

	// Close first so command-initiated focus (search input, modals) wins over the
	// caller's list-focus restoration; then invoke the command.
	function runCommand(cmd: PaletteCommand) {
		onClose();
		cmd.run();
	}

	function runSelected() {
		const cmd = results[selectedIndex];
		if (!cmd) {
			onClose();
			return;
		}
		runCommand(cmd);
	}

	function onKeydown(e: KeyboardEvent) {
		if (e.key === "ArrowDown" || (e.ctrlKey && e.key === "n")) {
			e.preventDefault();
			move(1);
		} else if (e.key === "ArrowUp" || (e.ctrlKey && e.key === "p")) {
			e.preventDefault();
			move(-1);
		} else if (e.key === "Enter") {
			e.preventDefault();
			runSelected();
		} else if (e.key === "Escape") {
			e.preventDefault();
			e.stopPropagation();
			onClose();
		}
	}

	function closeFromBackdrop(e: MouseEvent) {
		if (e.target === e.currentTarget) onClose();
	}
</script>

<div
	class="scrim"
	role="button"
	tabindex="-1"
	onclick={closeFromBackdrop}
	onkeydown={() => {}}
>
	<div class="palette" role="dialog" aria-modal="true" aria-label="Command palette">
		<div class="pal-input">
			<span class="prompt" aria-hidden="true"></span>
			<input
				bind:this={inputEl}
				bind:value={query}
				onkeydown={onKeydown}
				type="text"
				autocomplete="off"
				autocapitalize="off"
				autocorrect="off"
				spellcheck="false"
				{placeholder}
				aria-label="Command palette"
				role="combobox"
				aria-expanded="true"
				aria-controls={LISTBOX_ID}
				aria-activedescendant={results.length ? optionId(selectedIndex) : undefined}
			/>
			<span class="count">{results.length} / {commands.length}</span>
		</div>

		<div class="pal-list" bind:this={listEl} id={LISTBOX_ID} role="listbox" aria-label="Commands">
			{#each results as cmd, i (cmd.id)}
				<div
					id={optionId(i)}
					class="pal-item"
					class:sel={i === selectedIndex}
					role="option"
					aria-selected={i === selectedIndex}
					tabindex="-1"
					onclick={() => runCommand(cmd)}
					onmouseenter={() => (selectedIndex = i)}
					onkeydown={() => {}}
				>
					<span class="ptext">{cmd.title}</span>
					{#if cmd.hint}<span class="pkey">{cmd.hint}</span>{/if}
				</div>
			{:else}
				<div class="pal-empty">No matching commands</div>
			{/each}
		</div>

		<div class="pal-foot">
			<span><b>↑↓</b> navigate</span>
			<span><b>enter</b> run</span>
			<span><b>esc</b> close</span>
		</div>
	</div>
</div>

<style>
	.scrim {
		position: fixed;
		inset: 0;
		z-index: 200;
		background: color-mix(in srgb, var(--c0) 62%, transparent);
		display: grid;
		align-items: start;
		justify-items: center;
		padding-top: 64px;
	}
	.palette {
		width: min(560px, 88%);
		background: var(--panel-2);
		border: 1px solid var(--border-strong);
		border-radius: var(--radius);
		box-shadow: 0 18px 60px -12px var(--c0);
		overflow: hidden;
		display: flex;
		flex-direction: column;
		max-height: calc(100vh - 128px);
	}
	.pal-input {
		display: flex;
		align-items: center;
		gap: 10px;
		padding: 12px 16px;
		border-bottom: 1px solid var(--border);
		flex-shrink: 0;
	}
	.pal-input .prompt {
		color: var(--accent);
	}
	.pal-input input {
		flex: 1;
		background: transparent;
		border: none;
		outline: none;
		color: var(--fg);
		font: inherit;
		padding: 0;
		width: 0;
		min-width: 0;
	}
	.pal-input input::placeholder {
		color: var(--muted-2);
	}
	.pal-input .count {
		margin-left: auto;
		color: var(--muted-2);
		font-size: 11px;
		flex-shrink: 0;
	}
	.pal-list {
		padding: 6px;
		overflow-y: auto;
		/* ~12 rows visible before scrolling (row ≈ 30px). */
		max-height: 360px;
	}
	.pal-item {
		display: flex;
		align-items: center;
		gap: 12px;
		padding: 7px 10px;
		border-radius: var(--radius);
		color: var(--fg);
		cursor: pointer;
	}
	.pal-item .ptext {
		flex: 1;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.pal-item .pkey {
		color: var(--muted-2);
		font-size: 10.5px;
		flex-shrink: 0;
	}
	.pal-item.sel {
		background: var(--sel-bg);
	}
	.pal-item.sel .ptext {
		color: var(--sel-fg);
	}
	.pal-item.sel .pkey {
		color: color-mix(in srgb, var(--sel-fg) 78%, transparent);
	}
	.pal-empty {
		padding: 12px 10px;
		color: var(--muted);
		font-size: 12px;
	}
	.pal-foot {
		display: flex;
		gap: 14px;
		padding: 7px 14px;
		border-top: 1px solid var(--border);
		color: var(--muted-2);
		font-size: 10.5px;
		flex-shrink: 0;
	}
	.pal-foot b {
		color: var(--muted);
		font-weight: 500;
	}
</style>
