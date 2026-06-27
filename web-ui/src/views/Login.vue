<script setup>
import { ref } from 'vue'
import { useRouter } from 'vue-router'
import { useAuth } from '../store'

const auth = useAuth()
const router = useRouter()
const username = ref('admin')
const password = ref('')
const error = ref('')
const loading = ref(false)

async function submit() {
  error.value = ''
  loading.value = true
  try {
    await auth.login(username.value, password.value)
    router.push('/dashboard')
  } catch (e) {
    error.value = e.message
  } finally {
    loading.value = false
  }
}
</script>

<template>
  <div class="h-full flex items-center justify-center bg-slate-100">
    <div class="bg-white p-8 rounded-lg shadow w-80">
      <h1 class="text-xl font-bold mb-1 flex items-center gap-2">
        <span>🦀</span> MiniMask
      </h1>
      <p class="text-slate-500 text-sm mb-6">内网穿透管理后台</p>
      <form @submit.prevent="submit" class="space-y-4">
        <div>
          <label class="block text-xs text-slate-500 mb-1">用户名</label>
          <input v-model="username" class="w-full border rounded px-3 py-2 focus:outline-none focus:ring-2 focus:ring-slate-400" />
        </div>
        <div>
          <label class="block text-xs text-slate-500 mb-1">密码</label>
          <input v-model="password" type="password" class="w-full border rounded px-3 py-2 focus:outline-none focus:ring-2 focus:ring-slate-400" />
        </div>
        <div v-if="error" class="text-red-500 text-sm">{{ error }}</div>
        <button
          :disabled="loading"
          class="w-full bg-slate-900 text-white rounded py-2 hover:bg-slate-700 disabled:opacity-50"
        >
          {{ loading ? '登录中…' : '登录' }}
        </button>
      </form>
      <p class="text-xs text-slate-400 mt-4">默认账号 admin / admin，请在设置中尽快修改密码</p>
    </div>
  </div>
</template>
