<script setup>
import { ref, onMounted } from 'vue'
import { api } from '../api'

const clients = ref([])
const loading = ref(false)
const newName = ref('')
const error = ref('')
const createdToken = ref(null) // { id, name, token }
const copied = ref(false)

async function load() {
  loading.value = true
  try {
    clients.value = await api.get('/api/clients')
  } catch (e) {
    error.value = e.message
  } finally {
    loading.value = false
  }
}

async function createClient() {
  error.value = ''
  if (!newName.value.trim()) {
    error.value = '请输入客户端名称'
    return
  }
  try {
    const res = await api.post('/api/clients', { name: newName.value.trim() })
    createdToken.value = { id: res.client.id, name: res.client.name, token: res.token }
    copied.value = false
    newName.value = ''
    await load()
  } catch (e) {
    error.value = e.message
  }
}

async function toggle(c) {
  try {
    await api.patch(`/api/clients/${c.id}`, { enabled: !c.enabled })
    await load()
  } catch (e) {
    error.value = e.message
  }
}

async function regenerate(c) {
  if (!confirm(`确定要重新生成客户端「${c.name}」的 Token 吗？旧 Token 将立即失效。`)) return
  try {
    const res = await api.post(`/api/clients/${c.id}/regenerate-token`)
    createdToken.value = { id: c.id, name: c.name, token: res.token }
    copied.value = false
  } catch (e) {
    error.value = e.message
  }
}

async function remove(c) {
  if (!confirm(`确定删除客户端「${c.name}」及其所有端口映射？`)) return
  try {
    await api.del(`/api/clients/${c.id}`)
    await load()
  } catch (e) {
    error.value = e.message
  }
}

function copyToken() {
  navigator.clipboard.writeText(createdToken.value.token)
  copied.value = true
}

function downloadConfig(c) {
  const text = `# MiniMask client config (client: ${c.name})
# Run: minimask client --server <host:7443> --tls --id ${c.id} --token <TOKEN> --server-name localhost
# Token is shown ONCE above; keep it safe.`
  const blob = new Blob([text], { type: 'text/plain' })
  const url = URL.createObjectURL(blob)
  const a = document.createElement('a')
  a.href = url
  a.download = `minimask-client-${c.id}.txt`
  a.click()
  URL.revokeObjectURL(url)
}

onMounted(load)
</script>

<template>
  <div class="p-6">
    <h1 class="text-2xl font-bold mb-6">客户端管理</h1>

    <div v-if="createdToken" class="bg-emerald-50 border border-emerald-200 rounded p-4 mb-6">
      <div class="font-semibold text-emerald-800 mb-1">客户端「{{ createdToken.name }}」的 Token（仅显示一次，请妥善保存）</div>
      <code class="block bg-white border rounded p-2 mt-2 break-all text-sm">{{ createdToken.token }}</code>
      <div class="mt-2 flex gap-3">
        <button @click="copyToken" class="text-sm bg-emerald-600 text-white px-3 py-1 rounded">
          {{ copied ? '已复制 ✓' : '复制 Token' }}
        </button>
        <button @click="downloadConfig(createdToken)" class="text-sm border px-3 py-1 rounded">下载客户端配置</button>
        <button @click="createdToken = null" class="text-sm text-slate-500">关闭</button>
      </div>
    </div>

    <div class="bg-white p-4 rounded shadow mb-6 flex gap-3 items-end">
      <div class="flex-1">
        <label class="block text-xs text-slate-500 mb-1">新建客户端</label>
        <input v-model="newName" @keyup.enter="createClient" placeholder="名称，例如 my-pc" class="w-full border rounded px-3 py-2" />
      </div>
      <button @click="createClient" class="bg-slate-900 text-white px-4 py-2 rounded">创建</button>
    </div>

    <div v-if="error" class="text-red-500 text-sm mb-4">{{ error }}</div>

    <div class="bg-white rounded shadow overflow-hidden">
      <table class="w-full text-sm">
        <thead class="bg-slate-50 text-slate-500 text-left">
          <tr>
            <th class="px-4 py-3">名称</th>
            <th>客户端 ID</th>
            <th>Token 前缀</th>
            <th>映射数</th>
            <th>状态</th>
            <th>创建时间</th>
            <th class="text-right pr-4">操作</th>
          </tr>
        </thead>
        <tbody>
          <tr v-if="!clients.length && !loading">
            <td colspan="7" class="px-4 py-8 text-center text-slate-400">还没有客户端，点击上方创建</td>
          </tr>
          <tr v-for="c in clients" :key="c.id" class="border-t hover:bg-slate-50">
            <td class="px-4 py-3 font-medium">{{ c.name }}</td>
            <td class="font-mono text-xs text-slate-500">{{ c.id }}</td>
            <td class="font-mono text-xs text-slate-500">{{ c.token_prefix }}…</td>
            <td>{{ c.mappings.length }}</td>
            <td>
              <span :class="c.enabled ? 'bg-emerald-100 text-emerald-700' : 'bg-slate-100 text-slate-500'" class="px-2 py-0.5 rounded text-xs">
                {{ c.enabled ? '启用' : '禁用' }}
              </span>
            </td>
            <td class="text-slate-500 text-xs">{{ new Date(c.created_at).toLocaleString() }}</td>
            <td class="text-right pr-4 space-x-2 whitespace-nowrap">
              <button @click="toggle(c)" class="text-slate-600 hover:text-slate-900">{{ c.enabled ? '禁用' : '启用' }}</button>
              <button @click="regenerate(c)" class="text-amber-600 hover:text-amber-800">重置Token</button>
              <button @click="remove(c)" class="text-red-500 hover:text-red-700">删除</button>
            </td>
          </tr>
        </tbody>
      </table>
    </div>
  </div>
</template>
