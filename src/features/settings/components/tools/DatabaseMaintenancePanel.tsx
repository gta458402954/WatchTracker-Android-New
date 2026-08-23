interface DatabaseMaintenancePanelProps { vacuumStatus: string; onVacuum: () => void; }
export default function DatabaseMaintenancePanel({ vacuumStatus, onVacuum }: DatabaseMaintenancePanelProps) {
  return <div className="bg-white rounded-3xl p-6 shadow-sm border border-gray-100 space-y-4">
                <div className="flex items-center gap-2.5 border-b border-gray-50 pb-4">
                  <span className="text-2xl">🧹</span>
                  <div>
                    <h4 className="font-bold text-gray-800">数据库维护与体积优化</h4>
                    <p className="text-[11px] text-gray-400">对本地 SQLite 数据库进行整理优化，提高运行速度</p>
                  </div>
                </div>
                <button
                  onClick={onVacuum}
                  className="w-full py-2.5 rounded-xl border border-gray-200 hover:bg-gray-50 text-sm font-semibold text-gray-600 transition-colors shadow-sm flex items-center justify-center gap-2"
                >
                  🧹 立即运行 SQLite 数据库压缩 (VACUUM)
                </button>
                {vacuumStatus && <p className="text-xs text-center text-gray-500 font-medium mt-1">{vacuumStatus}</p>}
</div>;
}
