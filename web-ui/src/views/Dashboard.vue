<script setup>
import { computed } from 'vue'
import { useStats } from '../composables/useStats'

const stats = useStats()

function fmtBytes(n) {
  if (!n) return '0 B'
  const units = ['B', 'KB', 'MB', 'GB', 'TB']
  let i = 0
  let v = n
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024
    i++
  }
  return v.toFixed(v < 10 ? 1 : 0) + ' ' + units[i]
}

function fmtRate(n) {
  return fmtBytes(n) + '/s'
}

function timeAgo(iso) {
  if (!iso) return '-'
  const d = (Date.now() - new Date(iso).getTime()) / 1000
  if (d < 60) return Math.floor(d) + ' 秒'
  if (d < 3600) return Math.floor(d / 60) + ' 分钟'
  return Math.floor(d / 3600) + ' 小时'
}

// Build an SVG path for an area chart from a history array of {rate_in/rate_out}.
const W = 600
const H = 120
function pathFor(key) {
  const h = stats.history
  if (h.length < 2) return ''
  const max = Math.max(1, ...h.map((s) => Math.max(s.rate_in, s.rate_out)))
  const step = W / (h.length - 1)
  const pts = h.map((s, i) => `${(i * step).toFixed(1)},${(H - (s[key] / max) * H).toFixed(1)}`)
  return 'M0,' + H + ' L' + pts.join(' L') + ' L' + W + ',' + H + ' Z'
}

const inPath = computed(() => pathFor('rate_in'))
const outPath = computed(() => pathFor('rate_out'))
</script>

<template>
  <div class="p-6">
    <h1 class="text-2xl font-bold mb-6">仪表盘</h1>

    <div class="grid grid-cols-4 gap-4 mb-6">
      <div class="bg-white p-4 rounded shadow">
        <div class="text-sm text-slate-500">入站流量</div>
        <div class="text-2xl font-bold text-emerald-600">{{ fmtBytes(stats.bytes_in) }}</div>
      </div>
      <div class="bg-white p-4 rounded shadow">
        <div class="text-sm text-slate-500">出站流量</div>
        <div class="text-2xl font-bold text-sky-600">{{ fmtBytes(stats.bytes_out) }}</div>
      </div>
      <div class="bg-white p-4 rounded shadow">
        <div class="text-sm text-slate-500">当前连接</div>
        <div class="text-2xl font-bold text-amber-600">{{ stats.active_conns }}</div>
      </div>
      <div class="bg-white p-4 rounded shadow">
        <div class="text-sm text-slate-500">累计连接</div>
        <div class="text-2xl font-bold">{{ stats.total_conns }}</div>
      </div>
    </div>

    <div class="bg-white p-5 rounded shadow mb-6">
      <div class="flex items-center justify-between mb-3">
        <h2 class="font-semibold">实时带宽（最近 {{ stats.history.length }} 秒）</h2>
        <div class="text-sm flex gap-4">
          <span class="text-emerald-600">▲ 入 {{ fmtRate(stats.history.at(-1)?.rate_in || 0) }}</span>
          <span class="text-sky-600">▲ 出 {{ fmtRate(stats.history.at(-1)?.rate_out || 0) }}</span>
        </div>
      </div>
      <svg :viewBox="`0 0 ${W} ${H}`" class="w-full" preserveAspectRatio="none" style="height:160px">
        <path :d="inPath" fill="rgba(16,185,129,0.15)" stroke="#10b981" stroke-width="2" />
        <path :d="outPath" fill="rgba(56,189,248,0.12)" stroke="#38bdf8" stroke-width="2" />
      </svg>
    </div>

    <div class="bg-white p-5 rounded shadow">
      <h2 class="font-semibold mb-3">在线隧道会话</h2>
      <table class="w-full text-sm">
        <thead class="text-slate-500 text-left border-b">
          <tr>
            <th class="py-2">客户端</th>
            <th>远端地址</th>
            <th>已连接</th>
            <th>活跃连接</th>
          </tr>
        </thead>
        <tbody>
          <tr v-if="!stats.sessions.length">
            <td colspan="4" class="py-6 text-center text-slate-400">暂无在线客户端</td>
          </tr>
          <tr v-for="s in stats.sessions" :key="s.client_id" class="border-b">
            <td class="py-2 font-mono text-xs">{{ s.client_id }}</td>
            <td class="text-slate-600">{{ s.remote_addr }}</td>
            <td class="text-slate-600">{{ timeAgo(s.connected_at) }}</td>
            <td><span class="px-2 py-0.5 bg-amber-100 text-amber-700 rounded">{{ s.active_conns }}</span></td>
          </tr>
        </tbody>
      </table>
    </div>
  </div>
</template>
