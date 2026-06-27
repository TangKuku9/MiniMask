<script setup>
import { ref, onMounted } from 'vue'
import { api } from '../api'

const settings = ref(null)
const error = ref('')
const msg = ref('')
const pw = ref({ old_password: '', new_password: '', confirm: '' })

async function load() {
  try {
    settings.value = await api.get('/api/settings')
  } catch (e) {
    error.value = e.message
  }
}

async function changePassword() {
  error.value = ''
  msg.value = ''
  if (pw.value.new_password !== pw.value.confirm) {
    error.value = '两次输入的新密码不一致'
    return
  }
  try {
    await api.post('/api/settings/password', {
      old_password: pw.value.old_password,
      new_password: pw.value.new_password
    })
    msg.value = '密码已修改'
    pw.value = { old_password: '', new_password: '', confirm: '' }
  } catch (e) {
    error.value = e.message
  }
}

onMounted(load)
</script>

<template>
  <div class="p-6 max-w-3xl">
    <h1 class="text-2xl font-bold mb-6">系统设置</h1>

    <div v-if="settings" class="bg-white p-5 rounded shadow mb-6">
      <h2 class="font-semibold mb-3">服务信息</h2>
      <dl class="grid grid-cols-2 gap-y-2 text-sm">
        <dt class="text-slate-500">管理员账号</dt><dd>{{ settings.username }}</dd>
        <dt class="text-slate-500">隧道监听地址</dt><dd class="font-mono">{{ settings.tunnel_bind }} (TLS: {{ settings.tunnel_tls ? '是' : '否' }})</dd>
        <dt class="text-slate-500">Web 监听地址</dt><dd class="font-mono">{{ settings.web_bind }} (TLS: {{ settings.web_tls ? '是' : '否' }})</dd>
        <dt class="text-slate-500">最大客户端数</dt><dd>{{ settings.max_clients }}</dd>
        <dt class="text-slate-500">单客户端最大连接</dt><dd>{{ settings.max_conns_per_client }}</dd>
        <dt class="text-slate-500">数据目录</dt><dd class="font-mono text-xs">{{ settings.data_dir }}</dd>
      </dl>
    </div>

    <div class="bg-white p-5 rounded shadow">
      <h2 class="font-semibold mb-3">修改管理员密码</h2>
      <form @submit.prevent="changePassword" class="space-y-3 max-w-sm">
        <input v-model="pw.old_password" type="password" placeholder="当前密码" class="w-full border rounded px-3 py-2" />
        <input v-model="pw.new_password" type="password" placeholder="新密码（至少 6 位）" class="w-full border rounded px-3 py-2" />
        <input v-model="pw.confirm" type="password" placeholder="确认新密码" class="w-full border rounded px-3 py-2" />
        <div v-if="error" class="text-red-500 text-sm">{{ error }}</div>
        <div v-if="msg" class="text-emerald-600 text-sm">{{ msg }}</div>
        <button class="bg-slate-900 text-white px-4 py-2 rounded">保存</button>
      </form>
    </div>
  </div>
</template>
