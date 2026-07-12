<script lang="ts">
	import { mail } from "$lib/stores/mail.svelte";
	import { roleGlyph } from "$lib/format";
	import type { Folder } from "$lib/types";

	function selectFolder(f: Folder) {
		void mail.selectFolder(f);
	}
</script>

{#if !mail.unified && mail.selectedAccountId !== null}
	{@const acc = mail.accounts.find((a) => a.id === mail.selectedAccountId)}
	{@const folders = mail.foldersByAccount[mail.selectedAccountId] ?? []}
	{@const syncing = mail.syncStates[mail.selectedAccountId] === "syncing"}
	<aside class="folder-col" aria-label="Folders">
		{#if acc}
			<div class="col-head" title={acc.email}>
				<span class="acc-name">{acc.displayName || acc.email}</span>
			</div>
		{/if}
		<ul class="folder-list">
			{#each folders as folder (folder.id)}
				<li>
					<button
						class="folder-btn"
						class:active={mail.selectedFolderId === folder.id}
						onclick={() => selectFolder(folder)}
					>
						<span class="glyph">{roleGlyph(folder.role)}</span>
						<span class="fname">{folder.name}</span>
						{#if folder.unreadCount > 0}
							<span class="badge">{folder.unreadCount}</span>
						{/if}
					</button>
				</li>
			{/each}
			{#if folders.length === 0}
				<li class="empty">{syncing ? "Syncing folders…" : "No folders yet"}</li>
			{/if}
		</ul>
	</aside>
{/if}

<style>
	.folder-col {
		width: 190px;
		min-width: 190px;
		display: flex;
		flex-direction: column;
		height: 100%;
		background: var(--panel);
		border-right: 1px solid var(--border);
		overflow: hidden;
	}

	.col-head {
		padding: 8px 10px;
		border-bottom: 1px solid var(--border);
		color: var(--muted);
		font-size: 11px;
		text-transform: uppercase;
		letter-spacing: 0.4px;
		overflow: hidden;
	}

	.acc-name {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		display: block;
	}

	.folder-list {
		list-style: none;
		margin: 0;
		padding: 4px 0;
		overflow-y: auto;
		flex: 1;
	}

	.folder-btn {
		display: flex;
		align-items: center;
		gap: 8px;
		width: 100%;
		text-align: left;
		background: transparent;
		border: none;
		border-radius: 0;
		padding: 4px 10px 4px 14px;
		color: var(--fg);
	}

	.folder-btn:hover {
		background: var(--hover);
		border-color: transparent;
	}

	.folder-btn.active {
		background: var(--sel-bg);
		color: var(--sel-fg);
	}

	.glyph {
		flex: 0 0 auto;
		width: 1.2em;
		text-align: center;
		opacity: 0.85;
	}

	.fname {
		flex: 1;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		font-size: 13px;
	}

	.badge {
		flex: 0 0 auto;
		background: var(--accent-dim);
		color: var(--bg);
		font-size: 11px;
		line-height: 1;
		padding: 2px 6px;
		border-radius: 999px;
		font-weight: 600;
	}

	.folder-btn.active .badge {
		background: var(--accent);
	}

	.empty {
		padding: 6px 14px;
		color: var(--faint);
		font-size: 11px;
		font-style: italic;
	}
</style>
