<script setup>
import { useAuth } from '../store'
import { useRouter } from 'vue-router'

const auth = useAuth()
const router = useRouter()

async function logout() {
  await auth.logout()
  router.push('/login')
}

const nav = [
  { to: '/dashboard', label: '仪表盘', icon: '📊' },
  { to: '/clients', label: '客户端', icon: '🔌' },
  { to: '/mappings', label: '端口映射', icon: '🔗' },
  { to: '/logs', label: '审计日志', icon: '📜' },
  { to: '/settings', label: '系统设置', icon: '⚙️' }
]
</script>

<template>
  <div class="h-full flex bg-slate-50">
    <aside class="w-56 bg-slate-900 text-slate-300 flex flex-col">
      <div class="px-5 py-5 text-white font-bold text-lg flex items-center gap-2">
        <span>🦀</span><span>MiniMask</span>
      </div>
      <nav class="flex-1 px-2 space-y-1">
        <router-link
          v-for="item in nav"
          :key="item.to"
          :to="item.to"
          class="block px-3 py-2 rounded text-sm hover:bg-slate-800 transition-colors"
          active-class="bg-slate-800 text-white"
        >
          <span class="mr-2">{{ item.icon }}</span>{{ item.label }}
        </router-link>
      </nav>
      <div class="px-4 py-4 border-t border-slate-800 text-xs">
        <div class="text-slate-400">已登录</div>
        <div class="text-slate-200 font-medium">{{ auth.user?.username }}</div>
        <button @click="logout" class="mt-2 text-red-400 hover:text-red-300">退出登录</button>
      </div>
    </aside>
    <main class="flex-1 overflow-auto">
      <router-view />
    </main>
  </div>
</template>
