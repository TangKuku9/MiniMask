<script setup>
import { ref, onMounted } from 'vue'
import { api } from '../api'

const mappings = ref([])
const clients = ref([])
const error = ref('')
const form = ref({ client_id: '', name: '', remote_port: '', local_addr: '127.0.0.1:' })

async function load() {
  try {
    ;[mappings.value, clients.value] = await Promise.all([
      api.get('/api/mappings'),
      api.get('/api/clients')
    ])
    if (!form.value.client_id && clients.value.length) {
      form.value.client_id = clients.value[0].id
    }
  } catch (e) {
    error.value = e.message
  }
}

async function create() {
  error.value = ''
  const port = parseInt(form.value.remote_port)
  if (!form.value.client_id) return (error.value = '请选择客户端')
  if (!port || port < 1 || port > 65535) return (error.value = '请输入合法的远端端口 (1-65535)')
  if (!form.value.local_addr.trim()) return (error.value = '请输入本地地址')
  try {
    await api.post('/api/mappings', {
      client_id: form.value.client_id,
      name: form.value.name.trim() || `port-${port}`,
      remote_port: port,
      local_addr: form.value.local_addr.trim()
    })
    form.value.name = ''
    form.value.remote_port = ''
    await load()
  } catch (e) {
    error.value = e.message
  }
}

async function toggle(m) {
  try {
    await api.patch(`/api/mappings/${m.id}`, { enabled: !m.enabled })
    await load()
  } catch (e) {
    error.value = e.message
  }
}

async function remove(m) {
  if (!confirm(`删除映射「${m.name}」(公网 :${m.remote_port} -> ${m.local_addr})？`)) return
  try {
    await api.del(`/api/mappings/${m.id}`)
    await load()
  } catch (e) {
    error.value = e.message
  }
}

onMounted(load)
</script>

<template>
  <div class="p-6">
    <h1 class="text-2xl font-bold mb-6">端口映射</h1>

    <div class="bg-white p-4 rounded shadow mb-6">
      <div class="grid grid-cols-5 gap-3 items-end">
        <div>
          <label class="block text-xs text-slate-500 mb-1">客户端</label>
          <select v-model="form.client_id" class="w-full border rounded px-2 py-2">
            <option v-for="c in clients" :key="c.id" :value="c.id">{{ c.name }}</option>
          </select>
        </div>
        <div>
          <label class="block text-xs text-slate-500 mb-1">名称</label>
          <input v-model="form.name" placeholder="例如 web" class="w-full border rounded px-2 py-2" />
        </div>
        <div>
          <label class="block text-xs text-slate-500 mb-1">公网端口</label>
          <input v-model="form.remote_port" type="number" placeholder="8080" class="w-full border rounded px-2 py-2" />
        </div>
        <div class="col-span-1">
          <label class="block text-xs text-slate-500 mb-1">本地地址</label>
          <input v-model="form.local_addr" placeholder="127.0.0.1:80" class="w-full border rounded px-2 py-2" />
        </div>
        <div>
          <button @click="create" class="w-full bg-slate-900 text-white px-4 py-2 rounded">添加映射</button>
        </div>
      </div>
      <div v-if="error" class="text-red-500 text-sm mt-3">{{ error }}</div>
    </div>

    <div class="bg-white rounded shadow overflow-hidden">
      <table class="w-full text-sm">
        <thead class="bg-slate-50 text-slate-500 text-left">
          <tr>
            <th class="px-4 py-3">名称</th>
            <th>客户端</th>
            <th>公网端口</th>
            <th>本地地址</th>
            <th>状态</th>
            <th class="text-right pr-4">操作</th>
          </tr>
        </thead>
        <tbody>
          <tr v-if="!mappings.length">
            <td colspan="6" class="px-4 py-8 text-center text-slate-400">还没有端口映射</td>
          </tr>
          <tr v-for="m in mappings" :key="m.id" class="border-t hover:bg-slate-50">
            <td class="px-4 py-3 font-medium">{{ m.name }}</td>
            <td class="text-slate-600">{{ m.client_name }}</td>
            <td><span class="font-mono">:{{ m.remote_port }}</span></td>
            <td class="font-mono text-xs text-slate-600">{{ m.local_addr }}</td>
            <td>
              <span :class="m.enabled ? 'bg-emerald-100 text-emerald-700' : 'bg-slate-100 text-slate-500'" class="px-2 py-0.5 rounded text-xs">
                {{ m.enabled ? '启用' : '禁用' }}
              </span>
            </td>
            <td class="text-right pr-4 space-x-2">
              <button @click="toggle(m)" class="text-slate-600 hover:text-slate-900">{{ m.enabled ? '禁用' : '启用' }}</button>
              <button @click="remove(m)" class="text-red-500 hover:text-red-700">删除</button>
            </td>
          </tr>
        </tbody>
      </table>
    </div>
  </div>
</template>
