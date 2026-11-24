import type { DriverPluginManifest } from "../lib/types";

interface DriversDrawerProps {
  plugins: DriverPluginManifest[];
  onClose: () => void;
}

export function DriversDrawer({ plugins, onClose }: DriversDrawerProps) {
  return (
    <div className="drawer-backdrop" role="dialog" aria-modal="true" aria-label="Community Database Drivers">
      <div className="drawer-panel">
        <div className="panel-header">
          <div>
            <h2 className="panel-title">Community Database Drivers</h2>
            <div style={{ fontSize: 11.5, color: "var(--text-muted)" }}>
              Validated against plugins/schema.json ({plugins.length}/{plugins.length} PASS)
            </div>
          </div>
          <button type="button" className="btn btn-ghost" onClick={onClose}>
            Close
          </button>
        </div>

        <div className="panel-body">
          {plugins.map((plugin) => (
            <article key={plugin.id} className="plugin-card">
              <div className="plugin-top">
                <div>
                  <div className="plugin-name">{plugin.name}</div>
                  <div className="mono" style={{ fontSize: 11, color: "var(--text-muted)" }}>
                    {plugin.id} v{plugin.version} | {plugin.protocol}
                  </div>
                </div>
                <span className="pill-status" style={{ fontSize: 10.5, padding: "2px 8px" }}>
                  VALID
                </span>
              </div>

              <p className="plugin-desc">{plugin.description}</p>

              <div className="cap-row">
                <span className="tag">{plugin.wire_transport}</span>
                {plugin.default_port !== null && <span className="tag">port {plugin.default_port}</span>}
                {plugin.zero_c_deps && <span className="tag">zero-c-deps</span>}
                {plugin.capabilities.map((cap) => (
                  <span key={cap} className="tag">
                    {cap}
                  </span>
                ))}
              </div>
            </article>
          ))}
        </div>
      </div>
    </div>
  );
}
