const invoke = window.__TAURI__.core ? window.__TAURI__.core.invoke : window.__TAURI__.tauri.invoke;

const toggleClientBtn = document.getElementById('toggle-client-btn');
const btnText = document.getElementById('btn-text');
const statusIndicator = document.getElementById('status-indicator');
const statusText = document.getElementById('status-text');
const jobsList = document.getElementById('jobs-list');
const jobsCount = document.getElementById('jobs-count');
const emptyState = document.getElementById('empty-state');

const tabJobs = document.getElementById('tab-jobs');
const tabStats = document.getElementById('tab-stats');
const tabCompleted = document.getElementById('tab-completed');
const pageJobs = document.getElementById('page-jobs');
const pageStats = document.getElementById('page-stats');
const pageCompleted = document.getElementById('page-completed');
const monthSelect = document.getElementById('stats-month-select');

let isClientRunning = false;
let currentJobs = [];
let completedOrders = [];
let currentCompletedSearch = '';
let selectedMonth = 'current';
let updateInterval = null;
let availablePrinters = [];

async function fetchPrinters() {
    try {
        availablePrinters = await invoke('get_available_printers');
    } catch (e) {
        console.error('Failed to fetch printers:', e);
    }
}

async function init() {
    populateMonthSelect();
    await fetchPrinters();
    await checkClientStatus();
    await fetchJobs();
    await fetchStatistics();

    updateInterval = setInterval(async () => {
        await checkClientStatus();
        await fetchJobs();
    }, 2000);

    toggleClientBtn.addEventListener('click', handleToggleClient);

    document.getElementById('titlebar-minimize')?.addEventListener('click', () => invoke('minimize_window'));
    document.getElementById('titlebar-maximize')?.addEventListener('click', () => invoke('maximize_window'));
    document.getElementById('titlebar-close')?.addEventListener('click', () => invoke('close_window'));

    tabJobs?.addEventListener('click', () => switchTab('jobs'));
    tabStats?.addEventListener('click', () => switchTab('stats'));
    tabCompleted?.addEventListener('click', () => switchTab('completed'));

    document.getElementById('refresh-stats-btn')?.addEventListener('click', fetchStatistics);
    document.getElementById('refresh-completed-btn')?.addEventListener('click', fetchCompletedOrders);
    
    document.getElementById('completed-search-input')?.addEventListener('input', (e) => {
        currentCompletedSearch = e.target.value;
        renderCompletedOrders();
    });

    monthSelect?.addEventListener('change', (e) => {
        selectedMonth = e.target.value;
        fetchStatistics();
    });

    await fetchCompletedOrders();
}

function populateMonthSelect() {
    const monthSelect = document.getElementById('stats-month-select');
    if (!monthSelect) return;
    
    monthSelect.innerHTML = '';
    
    const now = new Date();
    const currentOption = document.createElement('option');
    currentOption.value = 'current';
    const currentMonthName = now.toLocaleString('en-US', { month: 'long' });
    currentOption.textContent = `${currentMonthName} ${now.getFullYear()} (Current)`;
    monthSelect.appendChild(currentOption);
    
    for (let i = 1; i <= 3; i++) {
        const d = new Date(now.getFullYear(), now.getMonth() - i, 1);
        const opt = document.createElement('option');
        const monthNum = String(d.getMonth() + 1).padStart(2, '0');
        opt.value = `${d.getFullYear()}-${monthNum}`;
        const monthName = d.toLocaleString('en-US', { month: 'long' });
        opt.textContent = `${monthName} ${d.getFullYear()}`;
        monthSelect.appendChild(opt);
    }
    
    const allOption = document.createElement('option');
    allOption.value = 'all';
    allOption.textContent = 'All Time';
    monthSelect.appendChild(allOption);
}

function switchTab(tab) {
    tabJobs?.classList.remove('active');
    tabStats?.classList.remove('active');
    tabCompleted?.classList.remove('active');
    
    pageJobs?.classList.remove('active');
    pageStats?.classList.remove('active');
    pageCompleted?.classList.remove('active');

    if (tab === 'jobs') {
        tabJobs?.classList.add('active');
        pageJobs?.classList.add('active');
    } else if (tab === 'stats') {
        tabStats?.classList.add('active');
        pageStats?.classList.add('active');
    } else if (tab === 'completed') {
        tabCompleted?.classList.add('active');
        pageCompleted?.classList.add('active');
    }
}

async function checkClientStatus() {
    try {
        isClientRunning = await invoke('get_client_status');
        updateHeaderUI();
    } catch (error) {
        console.error('Failed to check client status:', error);
    }
}

function updateHeaderUI() {
    if (isClientRunning) {
        statusIndicator.className = 'status-indicator status-running';
        statusText.textContent = 'Running';
        toggleClientBtn.className = 'btn btn-danger btn-sm';
        btnText.textContent = 'Stop Client';
    } else {
        statusIndicator.className = 'status-indicator status-stopped';
        statusText.textContent = 'Stopped';
        toggleClientBtn.className = 'btn btn-primary btn-sm';
        btnText.textContent = 'Start Client';
    }
}

async function handleToggleClient() {
    toggleClientBtn.disabled = true;
    try {
        if (isClientRunning) {
            await invoke('stop_client');
        } else {
            await invoke('start_client');
        }
        await checkClientStatus();
        await fetchJobs();
        await fetchStatistics();
    } catch (error) {
        console.error('Failed to toggle client:', error);
        alert(`Error: ${error}`);
    } finally {
        toggleClientBtn.disabled = false;
    }
}

function jobsAreDifferent(oldJobs, newJobs) {
    if (!oldJobs || !newJobs) return true;
    if (oldJobs.length !== newJobs.length) return true;
    for (let i = 0; i < oldJobs.length; i++) {
        const o = oldJobs[i];
        const n = newJobs[i];
        if (o.fileId !== n.fileId || o.status !== n.status) return true;
    }
    return false;
}

async function fetchJobs() {
    try {
        const jobs = await invoke('get_jobs');
        if (jobsAreDifferent(currentJobs, jobs)) {
            currentJobs = jobs;
            renderJobs(jobs);
        }
    } catch (error) {
        console.error('Failed to fetch jobs:', error);
    }
}

async function fetchStatistics() {
    try {
        let monthParam = selectedMonth;
        if (monthParam === 'current') {
            const now = new Date();
            monthParam = `${now.getFullYear()}-${String(now.getMonth() + 1).padStart(2, '0')}`;
        } else if (monthParam === 'all') {
            monthParam = null;
        }

        const statsJson = await invoke('get_stats', { month: monthParam });
        const stats = JSON.parse(statsJson);

        const count1sMono = stats["b/w single sided"]?.pages || 0;
        const count2sMono = stats["b/w double sided"]?.pages || 0;
        const count1sColor = stats["color single sided"]?.pages || 0;
        const count2sColor = stats["color double sided"]?.pages || 0;

        const price1sMono = count1sMono * 3;
        const price2sMono = count2sMono * 2;
        const price1sColor = count1sColor * 6;
        const price2sColor = count2sColor * 6;

        const net1sMono = price1sMono * 0.975;
        const net2sMono = price2sMono * 0.975;
        const net1sColor = price1sColor * 0.975;
        const net2sColor = price2sColor * 0.975;

        const totalCount = count1sMono + count2sMono + count1sColor + count2sColor;
        const totalPrice = price1sMono + price2sMono + price1sColor + price2sColor;
        const totalNet = net1sMono + net2sMono + net1sColor + net2sColor;
        const vendorPayable = 2 * (totalPrice - totalNet);

        document.getElementById('stat-1s-mono-count').textContent = count1sMono;
        document.getElementById('stat-1s-mono-price').textContent = `₹${price1sMono.toFixed(2)}`;
        document.getElementById('stat-1s-mono-net').textContent = `₹${net1sMono.toFixed(2)}`;

        document.getElementById('stat-2s-mono-count').textContent = count2sMono;
        document.getElementById('stat-2s-mono-price').textContent = `₹${price2sMono.toFixed(2)}`;
        document.getElementById('stat-2s-mono-net').textContent = `₹${net2sMono.toFixed(2)}`;

        document.getElementById('stat-1s-color-count').textContent = count1sColor;
        document.getElementById('stat-1s-color-price').textContent = `₹${price1sColor.toFixed(2)}`;
        document.getElementById('stat-1s-color-net').textContent = `₹${net1sColor.toFixed(2)}`;

        document.getElementById('stat-2s-color-count').textContent = count2sColor;
        document.getElementById('stat-2s-color-price').textContent = `₹${price2sColor.toFixed(2)}`;
        document.getElementById('stat-2s-color-net').textContent = `₹${net2sColor.toFixed(2)}`;

        document.getElementById('stat-total-count').textContent = totalCount;
        document.getElementById('stat-total-price').textContent = `₹${totalPrice.toFixed(2)}`;
        document.getElementById('stat-total-net').textContent = `₹${totalNet.toFixed(2)}`;

        document.getElementById('stat-vendor-payable').textContent = `₹${vendorPayable.toFixed(2)}`;
    } catch (error) {
        console.error('Failed to fetch statistics:', error);
    }
}

function renderJobs(jobs) {
    jobsCount.textContent = `${jobs.length} Job${jobs.length === 1 ? '' : 's'}`;

    if (jobs.length === 0) {
        jobsList.innerHTML = '';
        jobsList.appendChild(emptyState);
        return;
    }

    const newListContainer = document.createElement('div');

    jobs.forEach(job => {
        const statusClass = `status-${job.status.toLowerCase()}`;

        const row = document.createElement('div');
        row.className = 'job-row';
        row.dataset.fileId = job.fileId;

        row.innerHTML = `
            <div class="spec-item">
                <span class="job-label">File ID</span>
                <span class="job-id" title="${job.fileId}">${job.fileId}</span>
            </div>
            <div class="spec-item">
                <span class="job-label">Status</span>
                <span class="job-status ${statusClass}">
                    ${job.status}
                </span>
            </div>
            <div class="spec-item">
                <span class="job-label">Color Mode</span>
                <span class="spec-val">${job.attributes.color}</span>
            </div>
            <div class="spec-item">
                <span class="job-label">Copies</span>
                <span class="spec-val">${job.attributes.copies || '1'}</span>
            </div>
            <div class="spec-item">
                <span class="job-label">Paper Format</span>
                <span class="spec-val" title="${job.attributes.paperFormat || 'Default'}">${job.attributes.paperFormat || 'Default'}</span>
            </div>
            <div class="spec-item">
                <span class="job-label">Sides</span>
                <span class="spec-val">${job.attributes.sides || 'one-sided'}</span>
            </div>
            <div class="job-actions">
                ${job.status.toLowerCase() === 'stuck' ? `
                    <select class="custom-select requeue-select" data-id="${job.fileId}" required>
                        <option value="" disabled selected>Select Printer</option>
                        ${availablePrinters.map(p => `<option value="${p.uri}">${p.name}</option>`).join('')}
                    </select>
                    <button class="btn btn-primary requeue-btn" data-id="${job.fileId}">
                        Requeue
                    </button>
                ` : ''}
                <button class="btn btn-reprint reprint-btn" data-id="${job.fileId}">
                    Reprint Job
                </button>
            </div>
        `;

        newListContainer.appendChild(row);
    });

    jobsList.innerHTML = newListContainer.innerHTML;

    jobsList.querySelectorAll('.reprint-btn').forEach(btn => {
        btn.addEventListener('click', async (e) => {
            const fileId = e.target.dataset.id;
            btn.disabled = true;
            btn.textContent = 'Queuing...';
            try {
                await invoke('reprint_job', { fileId });
                await fetchJobs();
            } catch (error) {
                console.error('Reprint failed:', error);
                alert(`Reprint failed: ${error}`);
                btn.disabled = false;
                btn.textContent = 'Reprint Job';
            }
        });
    });

    jobsList.querySelectorAll('.requeue-btn').forEach(btn => {
        btn.addEventListener('click', async (e) => {
            const fileId = e.target.dataset.id;
            const row = e.target.closest('.job-row');
            const select = row.querySelector('.requeue-select');
            const printerUri = select.value;
            
            if (!printerUri) {
                alert('Please select a printer to requeue to.');
                return;
            }
            
            btn.disabled = true;
            btn.textContent = 'Queuing...';
            try {
                await invoke('requeue_to_printer', { fileId, printerUri });
                await fetchJobs();
            } catch (error) {
                console.error('Requeue failed:', error);
                alert(`Requeue failed: ${error}`);
                btn.disabled = false;
                btn.textContent = 'Requeue';
            }
        });
    });
}

async function fetchCompletedOrders() {
    const list = document.getElementById('completed-orders-list');
    if (!list) return;

    try {
        const jsonStr = await invoke('get_completed_orders');
        completedOrders = JSON.parse(jsonStr) || [];
        renderCompletedOrders();
    } catch (error) {
        console.error('Failed to fetch completed orders:', error);
        list.innerHTML = `<div class="empty-state">
            <p>Error loading completed orders.</p>
        </div>`;
    }
}

function renderCompletedOrders() {
    const list = document.getElementById('completed-orders-list');
    if (!list) return;

    const filtered = completedOrders.filter(order => {
        if (!currentCompletedSearch) return true;
        return order.id.toString().toLowerCase().includes(currentCompletedSearch.toLowerCase());
    });

    if (filtered.length === 0) {
        if (completedOrders.length === 0) {
            list.innerHTML = `<div class="empty-state">
                <p>No completed orders found.</p>
            </div>`;
        } else {
            list.innerHTML = `<div class="empty-state">
                <p>No orders match your search.</p>
            </div>`;
        }
        return;
    }

    list.innerHTML = '';
    filtered.forEach(order => {
        const row = document.createElement('div');
        row.className = 'completed-order-row';
        
        const infoDiv = document.createElement('div');
        infoDiv.className = 'job-info';
        infoDiv.innerHTML = `
            <div class="job-id">Order #${order.id}</div>
            <div class="job-meta">Ready for pickup</div>
        `;
        
        const actionsDiv = document.createElement('div');
        actionsDiv.className = 'job-actions';
        
        const collectBtn = document.createElement('button');
        collectBtn.className = 'btn btn-primary btn-sm';
        collectBtn.textContent = 'Mark Collected';
        collectBtn.onclick = () => markCollected(order.id);
        
        actionsDiv.appendChild(collectBtn);
        
        row.appendChild(infoDiv);
        row.appendChild(actionsDiv);
        list.appendChild(row);
    });
}

async function markCollected(orderId) {
    try {
        await invoke('mark_order_collected', { orderId: orderId.toString() });
        await fetchCompletedOrders();
    } catch (error) {
        console.error('Failed to mark order as collected:', error);
        alert('Failed to mark order as collected. Please try again.');
    }
}

document.addEventListener('DOMContentLoaded', init);
