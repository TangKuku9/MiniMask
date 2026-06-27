<script setup>
import { ref, onMounted, onUnmounted } from 'vue'
import { api } from '../api'

const logs = ref([])
const filter = ref('')
const levelFilter = ref('')
let timer = null

async function load() {
  try {
    logs.value = await api.get('/api/logs')
  } catch {
    /* ignore */
  }
}

const filtered = () =>
  logs.value.filter((l) => {
    if (levelFilter.value && l.level !== levelFilter.value) return false
    if (filter.value && !l.message.includes(filter.value) && !l.category.includes(filter.value)) return false
    return true
  })

const levelColor = (lvl) =>
  ({
    warn: 'bg-amber-100 text-amber-700',
    error: 'bg-red-100 text-red-700'
  }[lvl] || 'bg-slate-100 text-slate-600')

onMounted(() => {
  load()
  timer = setInterval(load, 4000)
})
onUnmounted(() => timer && clearInterval(timer))
</script>

<template>
  <div class="p-6">
    <div class="flex items-center justify-between mb-6">
      <h1 class="text-2xl font-bold">审计日志</h1>
      <button @click="load" class="text-sm border px-3 py-1 rounded">刷新</button>
    </div>

    <div class="bg-white p-3 rounded shadow mb-4 flex gap-3">
      <input v-model="filter" placeholder="按关键字过滤…" class="flex-1 border rounded px-3 py-1.5 text-sm" />
      <select v-model="levelFilter" class="border rounded px-2 py-1.5 text-sm">
        <option value="">全部级别</option>
        <option value="info">info</option>
        <option value="warn">warn</option>
        <option value="error">error</option>
      </select>
    </div>

    <div class="bg-white rounded shadow overflow-hidden">
      <table class="w-full text-sm">
        <thead class="bg-slate-50 text-slate-500 text-left">
          <tr>
            <th class="px-4 py-3 w-44">时间</th>
            <th class="w-20">级别</th>
            <th class="w-28">分类</th>
            <th>消息</th>
          </tr>
        </thead>
        <tbody>
          <tr v-if="!filtered().length">
            <td colspan="4" class="px-4 py-8 text-center text-slate-400">暂无日志</td>
          </tr>
          <tr v-for="(l, i) in filtered()" :key="i" class="border-t">
            <td class="px-4 py-2 text-slate-500 text-xs whitespace-nowrap">{{ new Date(l.ts).toLocaleString() }}</td>
            <td><span :class="levelColor(l.level)" class="px-2 py-0.5 rounded text-xs">{{ l.level }}</span></td>
            <td class="text-slate-600">{{ l.category }}</td>
            <td class="py-2">{{ l.message }}</td>
          </tr>
        </tbody>
      </table>
    </div>
  </div>
</template>
