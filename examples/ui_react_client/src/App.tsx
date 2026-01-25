import { useState, useEffect } from 'react';
import { ComponentRenderer } from './adk-ui-renderer/Renderer';
import { convertA2UIComponent } from './adk-ui-renderer/a2ui-converter';
import type { Component } from './adk-ui-renderer/types';
import './App.css';

interface Surface {
  surfaceId: string;
  components: Component[];
  dataModel: Record<string, unknown>;
}

interface Example {
  id: string;
  name: string;
  description: string;
  port: number;
}

const EXAMPLES: Example[] = [
  { id: 'ui_demo', name: 'UI Demo', description: 'Basic A2UI demo', port: 8080 },
  { id: 'ui_working_support', name: 'Support Intake', description: 'Support ticket system', port: 8081 },
  { id: 'ui_working_appointment', name: 'Appointments', description: 'Appointment booking', port: 8082 },
  { id: 'ui_working_events', name: 'Events', description: 'Event RSVP system', port: 8083 },
  { id: 'ui_working_facilities', name: 'Facilities', description: 'Work order system', port: 8084 },
  { id: 'ui_working_inventory', name: 'Inventory', description: 'Restock requests', port: 8085 },
];

function App() {
  const [surface, setSurface] = useState<Surface | null>(null);
  const [selectedExample, setSelectedExample] = useState<Example>(EXAMPLES[0]);
  const [isConnected, setIsConnected] = useState(false);

  useEffect(() => {
    let abortController = new AbortController();

    const startSession = async () => {
      try {
        console.log('Starting session for:', selectedExample.id, 'port:', selectedExample.port);
        setIsConnected(false);
        setSurface(null);

        const baseUrl = `http://localhost:${selectedExample.port}`;
        
        // Create session
        console.log('Creating session...');
        const sessionRes = await fetch(`${baseUrl}/api/apps/${selectedExample.id}/users/user1/sessions`, {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ state: {} }),
        });

        console.log('Session response:', sessionRes.status);
        if (!sessionRes.ok) {
          console.error('Failed to create session');
          return;
        }

        const session = await sessionRes.json();
        console.log('Session ID:', session.id);

        // Connect to SSE via POST
        const sseUrl = `${baseUrl}/api/run/${selectedExample.id}/user1/${session.id}`;
        console.log('Connecting to SSE:', sseUrl);
        
        const response = await fetch(sseUrl, {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ new_message: 'start' }),
        });

        if (!response.ok || !response.body) {
          console.error('Failed to connect to SSE');
          return;
        }

        setIsConnected(true);
        const reader = response.body.getReader();
        const decoder = new TextDecoder();

        const processStream = async () => {
          while (true) {
            const { done, value } = await reader.read();
            if (done) break;

            const text = decoder.decode(value);
            const lines = text.split('\n');

            for (const line of lines) {
              if (!line.startsWith('data: ')) continue;
              const eventData = line.slice(6).trim();
              if (!eventData || eventData === ':keep-alive') continue;

              try {
                const evt = JSON.parse(eventData);

                // Extract components from function response
                if (evt.content?.parts) {
                  for (const part of evt.content.parts) {
                    if (part.functionResponse?.name === 'render_screen') {
                      const response = part.functionResponse.response;
                      if (response.components) {
                        const componentsArray = typeof response.components === 'string' 
                          ? JSON.parse(response.components)
                          : response.components;
                        
                        const componentMap = new Map<string, any>();
                        componentsArray.forEach((comp: any) => {
                          const converted = convertA2UIComponent(comp);
                          if (converted) {
                            componentMap.set(converted.id, converted);
                          }
                        });
                        
                        const resolveChildren = (comp: any): any => {
                          if (comp.children && Array.isArray(comp.children)) {
                            return {
                              ...comp,
                              children: comp.children.map((childId: string) => {
                                const child = componentMap.get(childId);
                                return child ? resolveChildren(child) : null;
                              }).filter(Boolean)
                            };
                          }
                          return comp;
                        };
                        
                        const root = componentMap.get('root');
                        if (root) {
                          const resolvedRoot = resolveChildren(root);
                          setSurface({
                            surfaceId: response.surface_id || 'main',
                            components: [resolvedRoot],
                            dataModel: response.data_model || {},
                          });
                        }
                      }
                    }
                  }
                }
              } catch (e) {
                console.error('Failed to parse SSE event:', e);
              }
            }
          }
        };

        processStream().catch(console.error);
      } catch (error) {
        console.error('Failed to start session:', error);
      }
    };

    startSession();

    return () => {
      abortController.abort();
    };
  }, [selectedExample]);

  return (
    <div className="min-h-screen bg-gray-50 dark:bg-gray-900">
      {/* Header */}
      <div className="bg-white dark:bg-gray-800 border-b dark:border-gray-700 px-6 py-4">
        <div className="max-w-7xl mx-auto flex items-center justify-between">
          <div>
            <h1 className="text-2xl font-bold text-gray-900 dark:text-white">
              A2UI Examples
            </h1>
            <p className="text-sm text-gray-600 dark:text-gray-400 mt-1">
              {selectedExample.description}
            </p>
          </div>
          
          <div className="flex items-center gap-4">
            <select
              value={selectedExample.id}
              onChange={(e) => {
                const example = EXAMPLES.find(ex => ex.id === e.target.value);
                if (example) setSelectedExample(example);
              }}
              className="px-4 py-2 border border-gray-300 dark:border-gray-600 rounded-lg bg-white dark:bg-gray-700 text-gray-900 dark:text-white"
            >
              {EXAMPLES.map(ex => (
                <option key={ex.id} value={ex.id}>{ex.name}</option>
              ))}
            </select>
            
            <div className="flex items-center gap-2">
              <div className={`w-2 h-2 rounded-full ${isConnected ? 'bg-green-500' : 'bg-red-500'}`} />
              <span className="text-sm text-gray-600 dark:text-gray-400">
                {isConnected ? 'Connected' : 'Disconnected'}
              </span>
            </div>
          </div>
        </div>
      </div>

      {/* Content */}
      <div className="max-w-7xl mx-auto px-6 py-8">
        {surface && surface.components.length > 0 ? (
          <div className="bg-white dark:bg-gray-800 rounded-lg shadow-sm p-6">
            {surface.components.map((component, index) => (
              <ComponentRenderer key={index} component={component} />
            ))}
          </div>
        ) : (
          <div className="text-center py-12">
            <div className="text-gray-400 dark:text-gray-600 mb-4">
              {isConnected ? (
                <>
                  <div className="animate-spin rounded-full h-12 w-12 border-b-2 border-blue-500 mx-auto mb-4" />
                  <p>Waiting for UI...</p>
                </>
              ) : (
                <p>Connecting to {selectedExample.name}...</p>
              )}
            </div>
          </div>
        )}
      </div>
    </div>
  );
}

export default App;
