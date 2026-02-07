const { describe, it } = require('node:test');
const assert = require('node:assert/strict');

const { A2uiStore, applyProtocolPayload } = require('../dist/index.js');

function collectSurfaceState(surface) {
    const components = Array.from(surface.components.entries())
        .sort(([a], [b]) => a.localeCompare(b))
        .map(([id, value]) => ({ id, value }));
    return { components, dataModel: surface.dataModel };
}

describe('Protocol parity', () => {
    it('produces equivalent surface state for a2ui/ag_ui/mcp_apps payloads', () => {
        const surface = {
            surfaceId: 'main',
            catalogId: 'catalog',
            components: [
                { id: 'title', component: 'Text', text: 'Hello' },
                { id: 'root', component: 'Column', children: ['title'] },
            ],
            dataModel: { status: 'ok' },
        };

        const a2uiPayload = [
            JSON.stringify({ createSurface: { surfaceId: 'main', catalogId: 'catalog' } }),
            JSON.stringify({
                updateDataModel: {
                    surfaceId: 'main',
                    path: '/',
                    value: { status: 'ok' },
                },
            }),
            JSON.stringify({
                updateComponents: {
                    surfaceId: 'main',
                    components: surface.components,
                },
            }),
        ].join('\n');

        const agUiPayload = {
            protocol: 'ag_ui',
            events: [
                { type: 'RUN_STARTED', threadId: 'thread-main', runId: 'run-main' },
                {
                    type: 'CUSTOM',
                    name: 'adk.ui.surface',
                    value: {
                        format: 'adk-ui-surface-v1',
                        surface,
                    },
                },
                { type: 'RUN_FINISHED', threadId: 'thread-main', runId: 'run-main' },
            ],
        };

        const html = `<!doctype html><html><body><script id="adk-ui-surface" type="application/json">${JSON.stringify(surface)}</script></body></html>`;
        const mcpAppsPayload = {
            protocol: 'mcp_apps',
            payload: {
                resourceReadResponse: {
                    contents: [{ text: html }],
                },
            },
        };

        const a2uiStore = new A2uiStore();
        const agUiStore = new A2uiStore();
        const mcpAppsStore = new A2uiStore();

        applyProtocolPayload(a2uiStore, a2uiPayload);
        applyProtocolPayload(agUiStore, agUiPayload);
        applyProtocolPayload(mcpAppsStore, mcpAppsPayload);

        const a2uiState = collectSurfaceState(a2uiStore.getSurface('main'));
        const agUiState = collectSurfaceState(agUiStore.getSurface('main'));
        const mcpAppsState = collectSurfaceState(mcpAppsStore.getSurface('main'));

        assert.deepEqual(agUiState, a2uiState);
        assert.deepEqual(mcpAppsState, a2uiState);
    });
});
