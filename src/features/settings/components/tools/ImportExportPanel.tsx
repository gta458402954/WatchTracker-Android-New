interface ImportExportPanelProps { onImportLocal: () => void; onExport: () => void; }
export default function ImportExportPanel({ onImportLocal, onExport }: ImportExportPanelProps) {
  return <div className="bg-white rounded-3xl p-6 shadow-sm border border-gray-100 space-y-4">
                <div className="flex items-center gap-2.5 border-b border-gray-50 pb-4">
                  <span className="text-2xl">📁</span>
                  <div>
                    <h4 className="font-bold text-gray-800">本地文件导入与导出</h4>
                    <p className="text-[11px] text-gray-400">导出或读取本地备份的 watchtracker_backup.json 文件</p>
                  </div>
                </div>
                <div className="flex gap-3">
                  <button
                    onClick={onImportLocal}
                    className="flex-1 py-2.5 rounded-xl border border-gray-200 hover:bg-gray-50 text-sm font-semibold text-gray-600 transition-colors shadow-sm"
                  >
                    📤 导入本地 JSON
                  </button>
                  <button
                    onClick={() => void onExport()}
                    className="flex-1 py-2.5 rounded-xl border border-gray-200 hover:bg-gray-50 text-sm font-semibold text-gray-600 transition-colors shadow-sm"
                  >
                    📥 导出备份 JSON
                  </button>
                </div>
</div>;
}
