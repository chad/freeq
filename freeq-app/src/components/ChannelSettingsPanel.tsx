import { useState, useEffect, useCallback } from 'react';
import { useStore } from '../store';
import { rawCommand } from '../irc/client';

interface PolicyInfo {
  policy?: {
    channel_id: string;
    version: number;
    requirements: any;
    role_requirements: Record<string, any>;
    credential_endpoints: Record<string, {
      issuer: string;
      url: string;
      label: string;
      description?: string;
    }>;
    effective_at: string;
  };
  authority_set?: any;
}

// Presets for common credential types
const VERIFIER_PRESETS: {
  id: string;
  label: string;
  icon: string;
  description: string;
  credentialType: string;
  buildUrl: (param: string) => string;
  placeholder: string;
  paramLabel: string;
}[] = [
  {
    id: 'github_repo',
    label: 'GitHub Repo Collaborator',
    icon: '🐙',
    description: 'Require push access to a GitHub repository',
    credentialType: 'github_repo',
    buildUrl: (repo) => `/verify/github/start?repo=${encodeURIComponent(repo)}`,
    placeholder: 'owner/repo',
    paramLabel: 'Repository',
  },
  {
    id: 'github_org',
    label: 'GitHub Org Member',
    icon: '🏢',
    description: 'Require membership in a GitHub organization',
    credentialType: 'github_membership',
    buildUrl: (org) => `/verify/github/start?org=${encodeURIComponent(org)}`,
    placeholder: 'org-name',
    paramLabel: 'Organization',
  },
];

export function ChannelSettingsPanel() {
  const settingsChannel = useStore((s) => s.channelSettingsOpen);
  const setOpen = useStore((s) => s.setChannelSettingsOpen);

  if (!settingsChannel) return null;

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center p-4 bg-black/60 backdrop-blur-sm" onClick={() => setOpen(null)}>
      <div className="bg-bg-secondary border border-border rounded-xl shadow-2xl w-full max-w-lg max-h-[80vh] flex flex-col overflow-hidden" onClick={(e) => e.stopPropagation()}>
        <SettingsContent channel={settingsChannel} onClose={() => setOpen(null)} />
      </div>
    </div>
  );
}

function SettingsContent({ channel, onClose }: { channel: string; onClose: () => void }) {
  const [tab, setTab] = useState<'rules' | 'requirements' | 'roles'>('rules');
  const [policy, setPolicy] = useState<PolicyInfo | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  // Rules tab state
  const [rulesText, setRulesText] = useState('');
  const [saving, setSaving] = useState(false);

  // Requirements tab state
  const [showAddVerifier, setShowAddVerifier] = useState(false);
  const [selectedPreset, setSelectedPreset] = useState<string | null>(null);
  const [presetParam, setPresetParam] = useState('');
  const [addingVerifier, setAddingVerifier] = useState(false);

  // Roles tab state
  const [roleCredType, setRoleCredType] = useState('');
  const [roleName, setRoleName] = useState('op');
  const [addingRole, setAddingRole] = useState(false);

  const fetchPolicy = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const encoded = encodeURIComponent(channel);
      const res = await fetch(`/api/v1/policy/${encoded}`);
      if (res.status === 404) {
        setPolicy(null);
        setLoading(false);
        return;
      }
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      const data = await res.json();
      setPolicy(data);
    } catch (e: any) {
      setError(e.message);
    } finally {
      setLoading(false);
    }
  }, [channel]);

  useEffect(() => { fetchPolicy(); }, [fetchPolicy]);

  const handleSetRules = () => {
    if (!rulesText.trim()) return;
    setSaving(true);
    rawCommand(`POLICY ${channel} SET ${rulesText.trim()}`);
    setTimeout(() => {
      fetchPolicy();
      setSaving(false);
    }, 1000);
  };

  const handleAddVerifier = () => {
    const preset = VERIFIER_PRESETS.find((p) => p.id === selectedPreset);
    if (!preset || !presetParam.trim()) return;

    setAddingVerifier(true);
    const issuerDid = `did:web:${window.location.hostname}:verify`;
    const url = preset.buildUrl(presetParam.trim());
    const label = preset.label.replace(/ /g, '_');

    rawCommand(`POLICY ${channel} REQUIRE ${preset.credentialType} issuer=${issuerDid} url=${url} label=${label}`);

    setTimeout(() => {
      fetchPolicy();
      setAddingVerifier(false);
      setShowAddVerifier(false);
      setSelectedPreset(null);
      setPresetParam('');
    }, 1000);
  };

  const handleAddRole = () => {
    if (!roleCredType.trim()) return;
    setAddingRole(true);

    const issuerDid = `did:web:${window.location.hostname}:verify`;
    const requirement = JSON.stringify({
      type: 'PRESENT',
      credential_type: roleCredType.trim(),
      issuer: issuerDid,
    });

    rawCommand(`POLICY ${channel} SET-ROLE ${roleName} ${requirement}`);

    setTimeout(() => {
      fetchPolicy();
      setAddingRole(false);
      setRoleCredType('');
    }, 1000);
  };

  const handleClearPolicy = () => {
    if (!confirm(`Remove all policy from ${channel}? This cannot be undone.`)) return;
    rawCommand(`POLICY ${channel} CLEAR`);
    setTimeout(() => {
      fetchPolicy();
    }, 1000);
  };

  const tabs = [
    { id: 'rules' as const, label: 'Rules' },
    { id: 'requirements' as const, label: 'Verifiers' },
    { id: 'roles' as const, label: 'Roles' },
  ];

  return (
    <>
      {/* Header */}
      <div className="px-6 pt-5 pb-0 border-b border-border">
        <div className="flex items-center justify-between mb-4">
          <div>
            <h2 className="text-lg font-bold text-fg flex items-center gap-2">
              <svg className="w-4 h-4 text-fg-dim" viewBox="0 0 20 20" fill="currentColor">
                <path fillRule="evenodd" d="M11.49 3.17c-.38-1.56-2.6-1.56-2.98 0a1.532 1.532 0 01-2.286.948c-1.372-.836-2.942.734-2.106 2.106.54.886.061 2.042-.947 2.287-1.561.379-1.561 2.6 0 2.978a1.532 1.532 0 01.947 2.287c-.836 1.372.734 2.942 2.106 2.106a1.532 1.532 0 012.287.947c.379 1.561 2.6 1.561 2.978 0a1.533 1.533 0 012.287-.947c1.372.836 2.942-.734 2.106-2.106a1.533 1.533 0 01.947-2.287c1.561-.379 1.561-2.6 0-2.978a1.532 1.532 0 01-.947-2.287c.836-1.372-.734-2.942-2.106-2.106a1.532 1.532 0 01-2.287-.947zM10 13a3 3 0 100-6 3 3 0 000 6z" clipRule="evenodd" />
              </svg>
              Channel Settings
            </h2>
            <p className="text-sm text-fg-dim">{channel}</p>
          </div>
          <button onClick={onClose} className="text-fg-dim hover:text-fg p-1 -mr-1">
            <svg className="w-5 h-5" viewBox="0 0 20 20" fill="currentColor">
              <path fillRule="evenodd" d="M4.293 4.293a1 1 0 011.414 0L10 8.586l4.293-4.293a1 1 0 111.414 1.414L11.414 10l4.293 4.293a1 1 0 01-1.414 1.414L10 11.414l-4.293 4.293a1 1 0 01-1.414-1.414L8.586 10 4.293 5.707a1 1 0 010-1.414z" />
            </svg>
          </button>
        </div>
        {/* Tabs */}
        <div className="flex gap-1">
          {tabs.map((t) => (
            <button
              key={t.id}
              onClick={() => setTab(t.id)}
              className={`px-4 py-2 text-sm font-medium rounded-t-lg transition-colors ${
                tab === t.id
                  ? 'bg-bg text-fg border-b-2 border-accent'
                  : 'text-fg-dim hover:text-fg-muted'
              }`}
            >
              {t.label}
            </button>
          ))}
        </div>
      </div>

      {/* Body */}
      <div className="flex-1 overflow-y-auto px-6 py-4">
        {loading && (
          <div className="flex items-center justify-center py-8">
            <div className="w-6 h-6 border-2 border-accent border-t-transparent rounded-full animate-spin" />
          </div>
        )}

        {error && (
          <div className="bg-red-500/10 border border-red-500/20 rounded-lg p-3 text-sm text-red-400 mb-4">
            {error}
          </div>
        )}

        {!loading && tab === 'rules' && (
          <div className="space-y-4">
            {policy?.policy ? (
              <div className="bg-bg rounded-lg border border-border p-3">
                <div className="flex items-center justify-between mb-2">
                  <span className="text-xs text-fg-dim uppercase tracking-wide">Current Policy (v{policy.policy.version})</span>
                  <button
                    onClick={handleClearPolicy}
                    className="text-xs text-red-400 hover:text-red-300"
                  >
                    Remove policy
                  </button>
                </div>
                <p className="text-sm text-fg-muted">{describeRequirements(policy.policy.requirements)}</p>
              </div>
            ) : (
              <p className="text-sm text-fg-dim">No policy set. Add rules to gate channel access.</p>
            )}

            <div>
              <label className="block text-xs text-fg-dim uppercase tracking-wide mb-2">
                {policy?.policy ? 'Update rules' : 'Set channel rules'}
              </label>
              <textarea
                value={rulesText}
                onChange={(e) => setRulesText(e.target.value)}
                placeholder="e.g., By participating you agree to our Code of Conduct."
                rows={3}
                className="w-full bg-bg border border-border rounded-lg p-3 text-sm text-fg placeholder-fg-dim resize-none focus:outline-none focus:border-accent"
              />
              <button
                onClick={handleSetRules}
                disabled={!rulesText.trim() || saving}
                className="mt-2 px-4 py-1.5 text-sm font-medium bg-accent text-bg rounded-lg hover:bg-accent/90 disabled:opacity-50 disabled:cursor-not-allowed"
              >
                {saving ? 'Saving…' : policy?.policy ? 'Update Policy' : 'Set Policy'}
              </button>
            </div>
          </div>
        )}

        {!loading && tab === 'requirements' && (
          <div className="space-y-4">
            {!policy?.policy && (
              <div className="bg-yellow-500/10 border border-yellow-500/20 rounded-lg p-3 text-sm text-yellow-400">
                Set channel rules first before adding verifiers.
              </div>
            )}

            {/* Existing credential endpoints */}
            {policy?.policy?.credential_endpoints && Object.keys(policy.policy.credential_endpoints).length > 0 && (
              <div>
                <label className="block text-xs text-fg-dim uppercase tracking-wide mb-2">Active verifiers</label>
                <div className="space-y-2">
                  {Object.entries(policy.policy.credential_endpoints).map(([type, ep]) => (
                    <div key={type} className="bg-bg border border-border rounded-lg p-3 flex items-center gap-3">
                      <div className="w-8 h-8 rounded-lg bg-bg-tertiary flex items-center justify-center text-lg">
                        {type.includes('github') ? '🐙' : '🔑'}
                      </div>
                      <div className="flex-1 min-w-0">
                        <p className="text-sm font-medium text-fg">{ep.label}</p>
                        <p className="text-xs text-fg-dim truncate">{type} · {ep.issuer}</p>
                      </div>
                      <span className="text-xs text-green-400 bg-green-500/10 px-2 py-0.5 rounded-full">Active</span>
                    </div>
                  ))}
                </div>
              </div>
            )}

            {/* Add verifier */}
            {policy?.policy && !showAddVerifier && (
              <button
                onClick={() => setShowAddVerifier(true)}
                className="w-full p-3 border border-dashed border-border rounded-lg text-sm text-fg-dim hover:border-accent hover:text-accent transition-colors"
              >
                + Add credential verifier
              </button>
            )}

            {showAddVerifier && (
              <div className="bg-bg border border-border rounded-lg p-4 space-y-3">
                <label className="block text-xs text-fg-dim uppercase tracking-wide">Choose verifier type</label>
                <div className="grid grid-cols-1 gap-2">
                  {VERIFIER_PRESETS.map((preset) => (
                    <button
                      key={preset.id}
                      onClick={() => { setSelectedPreset(preset.id); setPresetParam(''); }}
                      className={`text-left p-3 rounded-lg border transition-colors ${
                        selectedPreset === preset.id
                          ? 'border-accent bg-accent/5'
                          : 'border-border hover:border-fg-dim'
                      }`}
                    >
                      <div className="flex items-center gap-2">
                        <span className="text-lg">{preset.icon}</span>
                        <span className="text-sm font-medium text-fg">{preset.label}</span>
                      </div>
                      <p className="text-xs text-fg-dim mt-1 ml-7">{preset.description}</p>
                    </button>
                  ))}
                </div>

                {selectedPreset && (
                  <div className="pt-2">
                    {(() => {
                      const preset = VERIFIER_PRESETS.find((p) => p.id === selectedPreset)!;
                      return (
                        <>
                          <label className="block text-xs text-fg-dim mb-1">{preset.paramLabel}</label>
                          <input
                            value={presetParam}
                            onChange={(e) => setPresetParam(e.target.value)}
                            placeholder={preset.placeholder}
                            className="w-full bg-bg-secondary border border-border rounded-lg px-3 py-2 text-sm text-fg placeholder-fg-dim focus:outline-none focus:border-accent"
                            onKeyDown={(e) => e.key === 'Enter' && handleAddVerifier()}
                          />
                        </>
                      );
                    })()}
                  </div>
                )}

                <div className="flex justify-end gap-2 pt-1">
                  <button
                    onClick={() => { setShowAddVerifier(false); setSelectedPreset(null); }}
                    className="px-3 py-1.5 text-xs text-fg-dim hover:text-fg"
                  >
                    Cancel
                  </button>
                  <button
                    onClick={handleAddVerifier}
                    disabled={!selectedPreset || !presetParam.trim() || addingVerifier}
                    className="px-4 py-1.5 text-xs font-medium bg-accent text-bg rounded-lg hover:bg-accent/90 disabled:opacity-50 disabled:cursor-not-allowed"
                  >
                    {addingVerifier ? 'Adding…' : 'Add Verifier'}
                  </button>
                </div>
              </div>
            )}
          </div>
        )}

        {!loading && tab === 'roles' && (
          <div className="space-y-4">
            {!policy?.policy && (
              <div className="bg-yellow-500/10 border border-yellow-500/20 rounded-lg p-3 text-sm text-yellow-400">
                Set channel rules first before configuring roles.
              </div>
            )}

            {/* Existing role requirements */}
            {policy?.policy?.role_requirements && Object.keys(policy.policy.role_requirements).length > 0 && (
              <div>
                <label className="block text-xs text-fg-dim uppercase tracking-wide mb-2">Role escalation rules</label>
                <div className="space-y-2">
                  {Object.entries(policy.policy.role_requirements).map(([role, req]) => (
                    <div key={role} className="bg-bg border border-border rounded-lg p-3">
                      <div className="flex items-center gap-2 mb-1">
                        {(role === 'op' || role === 'admin' || role === 'owner') && (
                          <span className="text-yellow-400 text-xs">⚡</span>
                        )}
                        <span className="text-sm font-medium text-fg">{role}</span>
                        <span className="text-xs text-fg-dim">→ {role === 'op' || role === 'admin' || role === 'owner' ? '+o' : role === 'voice' ? '+v' : 'no mode'}</span>
                      </div>
                      <p className="text-xs text-fg-dim ml-5">{describeRequirements(req)}</p>
                    </div>
                  ))}
                </div>
              </div>
            )}

            {/* Add role */}
            {policy?.policy && (
              <div className="bg-bg border border-border rounded-lg p-4 space-y-3">
                <label className="block text-xs text-fg-dim uppercase tracking-wide">Auto-assign role by credential</label>

                <div className="grid grid-cols-2 gap-2">
                  <div>
                    <label className="block text-xs text-fg-dim mb-1">Role</label>
                    <select
                      value={roleName}
                      onChange={(e) => setRoleName(e.target.value)}
                      className="w-full bg-bg-secondary border border-border rounded-lg px-3 py-2 text-sm text-fg focus:outline-none focus:border-accent"
                    >
                      <option value="op">Op (+o)</option>
                      <option value="voice">Voice (+v)</option>
                      <option value="admin">Admin (+o)</option>
                    </select>
                  </div>
                  <div>
                    <label className="block text-xs text-fg-dim mb-1">Requires credential</label>
                    <select
                      value={roleCredType}
                      onChange={(e) => setRoleCredType(e.target.value)}
                      className="w-full bg-bg-secondary border border-border rounded-lg px-3 py-2 text-sm text-fg focus:outline-none focus:border-accent"
                    >
                      <option value="">Select…</option>
                      {policy?.policy?.credential_endpoints && Object.keys(policy.policy.credential_endpoints).map((type) => (
                        <option key={type} value={type}>{type}</option>
                      ))}
                    </select>
                  </div>
                </div>

                <div className="flex justify-end">
                  <button
                    onClick={handleAddRole}
                    disabled={!roleCredType || addingRole}
                    className="px-4 py-1.5 text-xs font-medium bg-accent text-bg rounded-lg hover:bg-accent/90 disabled:opacity-50 disabled:cursor-not-allowed"
                  >
                    {addingRole ? 'Adding…' : 'Add Role Rule'}
                  </button>
                </div>
              </div>
            )}
          </div>
        )}
      </div>
    </>
  );
}

/** Describe a requirement tree as human-readable text. */
function describeRequirements(req: any): string {
  if (!req) return 'None';
  switch (req.type) {
    case 'ACCEPT':
      return `Accept rules (hash: ${req.hash?.slice(0, 12)}…)`;
    case 'PRESENT':
      return `Credential: ${req.credential_type}${req.issuer ? ` from ${req.issuer.slice(0, 30)}…` : ''}`;
    case 'PROVE':
      return `Prove: ${req.proof_type}`;
    case 'ALL':
      return (req.requirements || []).map(describeRequirements).join(' AND ');
    case 'ANY':
      return (req.requirements || []).map(describeRequirements).join(' OR ');
    case 'NOT':
      return `NOT (${describeRequirements(req.requirement)})`;
    default:
      return JSON.stringify(req).slice(0, 60);
  }
}
