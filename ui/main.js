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
const tabSettings = document.getElementById('tab-settings');
const pageJobs = document.getElementById('page-jobs');
const pageStats = document.getElementById('page-stats');
const pageCompleted = document.getElementById('page-completed');
const pageSettings = document.getElementById('page-settings');
const monthSelect = document.getElementById('stats-month-select');

let isClientRunning = false;
let currentJobs = [];
let completedOrders = [];
let currentCompletedSearch = '';
let selectedMonth = 'current';
let updateInterval = null;
let timerInterval = null;
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

    timerInterval = setInterval(tickTimers, 1000);

    toggleClientBtn.addEventListener('click', handleToggleClient);

    document.getElementById('titlebar-minimize')?.addEventListener('click', () => invoke('minimize_window'));
    document.getElementById('titlebar-maximize')?.addEventListener('click', () => invoke('maximize_window'));
    document.getElementById('titlebar-close')?.addEventListener('click', () => invoke('close_window'));

    tabJobs?.addEventListener('click', () => switchTab('jobs'));
    tabStats?.addEventListener('click', () => switchTab('stats'));
    tabCompleted?.addEventListener('click', () => switchTab('completed'));
    tabSettings?.addEventListener('click', () => switchTab('settings'));

    document.getElementById('refresh-stats-btn')?.addEventListener('click', fetchStatistics);
    document.getElementById('refresh-completed-btn')?.addEventListener('click', fetchCompletedOrders);
    document.getElementById('refresh-printers-btn')?.addEventListener('click', fetchPrinterList);

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

    const options = [
        { value: 'current', label: 'Current Month' },
        { value: 'past', label: 'Past Month' },
        { value: 'three', label: 'Last 3 Months' },
        { value: 'all', label: 'All Time' }
    ];

    options.forEach(optData => {
        const opt = document.createElement('option');
        opt.value = optData.value;
        opt.textContent = optData.label;
        monthSelect.appendChild(opt);
    });
}

function switchTab(tab) {
    [tabJobs, tabStats, tabCompleted, tabSettings].forEach(t => t?.classList.remove('active'));
    [pageJobs, pageStats, pageCompleted, pageSettings].forEach(p => p?.classList.remove('active'));

    if (tab === 'jobs') {
        tabJobs?.classList.add('active');
        pageJobs?.classList.add('active');
    } else if (tab === 'stats') {
        tabStats?.classList.add('active');
        pageStats?.classList.add('active');
    } else if (tab === 'completed') {
        tabCompleted?.classList.add('active');
        pageCompleted?.classList.add('active');
    } else if (tab === 'settings') {
        tabSettings?.classList.add('active');
        pageSettings?.classList.add('active');
        fetchPrinterList();
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
        if (monthParam === 'all') {
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

    const frag = document.createDocumentFragment();
    jobs.forEach(job => {
        const status = job.status.toLowerCase();
        const row = document.createElement('div');
        row.className = 'job-row-new';
        row.dataset.fileId = job.fileId;

        const titleLabel = job.orderId
            ? `${job.orderId} <span class="dim">— ${job.fileId}</span>`
            : job.fileId;

        const a = job.attributes;
        const pills = [
            a.color === 'Color' ? 'Color' : 'B&W',
            a.sides || 'one-sided',
            a.copies > 1 ? `×${a.copies}` : null,
            a.numberUp > 1 ? `${a.numberUp}-up` : null,
            a.paperFormat || null,
            a.pageRanges ? `pp ${a.pageRanges}` : null,
            a.orientation || null,
            a.printScaling || null,
            a.targetPrinter ? `${a.targetPrinter}` : null,
        ].filter(Boolean);

        const isStuck = status === 'stuck' || status === 'failed';

        // Timeout limits per status (seconds)
        const timeoutMap = { queued: 30, processing: 120 };
        const limit = timeoutMap[status] ?? null;
        const hasTimer = limit !== null;

        row.dataset.updatedAt = job.updatedAt;
        row.dataset.status = status;
        row.dataset.limit = limit ?? '';

        row.innerHTML = `
            <div class="job-row-header">
                <div style="display:flex;align-items:center;gap:0.5rem;min-width:0;flex:1">
                    <span class="job-status-dot dot-${status}"></span>
                    <span class="job-row-title">${titleLabel}</span>
                </div>
                <div class="job-actions">
                    ${isStuck ? `
                        <select class="custom-select requeue-select" data-id="${job.fileId}" required style="font-size:0.75rem;padding:0.3rem 1.5rem 0.3rem 0.6rem">
                            <option value="" disabled selected>Select Printer</option>
                            ${availablePrinters.map(p => `<option value="${p.uri}">${p.name}</option>`).join('')}
                        </select>
                        <button class="btn btn-primary btn-sm requeue-btn" data-id="${job.fileId}">Requeue</button>
                    ` : ''}
                    <button class="btn-reprint reprint-btn" data-id="${job.fileId}">Reprint</button>
                </div>
            </div>
            <div class="job-pills">${pills.map(p => `<span class="pill">${p}</span>`).join('')}</div>
            ${hasTimer ? `
            <div class="job-timer">
                <div class="job-timer-bar-wrap">
                    <div class="job-timer-bar" data-limit="${limit}"></div>
                </div>
                <span class="job-timer-label">0s / ${limit}s</span>
            </div>` : ''}
        `;
        frag.appendChild(row);
    });

    jobsList.innerHTML = '';
    jobsList.appendChild(frag);

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
                btn.textContent = 'Reprint';
            }
        });
    });

    jobsList.querySelectorAll('.requeue-btn').forEach(btn => {
        btn.addEventListener('click', async (e) => {
            const fileId = e.target.dataset.id;
            const row = e.target.closest('.job-row-new');
            const select = row.querySelector('.requeue-select');
            const printerUri = select.value;
            if (!printerUri) { alert('Please select a printer.'); return; }
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

async function fetchPrinterList() {
    const list = document.getElementById('printers-list');
    if (!list) return;
    try {
        const printers = await invoke('get_printer_list');
        availablePrinters = printers;
        list.innerHTML = '';
        if (!printers.length) {
            list.innerHTML = '<div class="empty-state"><p>No printers found</p></div>';
            return;
        }
        printers.forEach(p => {
            const card = document.createElement('div');
            card.className = 'printer-card';
            const isPaused = p.paused;
            card.innerHTML = `
                <div class="printer-card-info">
                    <div class="printer-card-meta">
                        <div class="printer-card-name">${p.name}</div>
                        <span class="pill">${p.colorMode === 'Color' ? 'Color' : 'Monochrome'}</span>
                        ${isPaused ? '<span class="pill" style="background:#fff3cd;border-color:#ffc107;color:#856404">Paused</span>' : '<span class="pill" style="background:#d1fae5;border-color:#6ee7b7;color:#065f46">Active</span>'}
                    </div>
                </div>
                <button class="printer-toggle-btn ${isPaused ? 'paused' : 'active'}" data-uri="${p.uri}" data-paused="${isPaused}">
                    ${isPaused ? 'Resume' : 'Pause'}
                </button>
            `;
            list.appendChild(card);
        });

        list.querySelectorAll('.printer-toggle-btn').forEach(btn => {
            btn.addEventListener('click', async () => {
                const uri = btn.dataset.uri;
                const paused = btn.dataset.paused === 'true';
                btn.disabled = true;
                try {
                    if (paused) {
                        await invoke('unpause_printer', { uri });
                    } else {
                        await invoke('pause_printer', { uri });
                    }
                    await fetchPrinterList();
                } catch (e) {
                    alert(`Failed: ${e}`);
                    btn.disabled = false;
                }
            });
        });
    } catch (e) {
        list.innerHTML = `<div class="empty-state"><p>Error loading printers: ${e}</p></div>`;
    }
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

function tickTimers() {
    const now = Math.floor(Date.now() / 1000);
    document.querySelectorAll('.job-row-new').forEach(row => {
        const updatedAt = parseInt(row.dataset.updatedAt, 10);
        const limit = parseInt(row.dataset.limit, 10);

        if (!isNaN(updatedAt) && !isNaN(limit)) {
            const elapsed = Math.max(0, now - updatedAt);
            const displayElapsed = Math.min(elapsed, limit);
            const percent = Math.min((displayElapsed / limit) * 100, 100);

            const bar = row.querySelector('.job-timer-bar');
            const label = row.querySelector('.job-timer-label');

            if (bar && label) {
                bar.style.width = `${percent}%`;
                label.textContent = `${displayElapsed}s / ${limit}s`;

                if (percent >= 80) {
                    bar.style.backgroundColor = 'hsl(0, 78%, 56%)'; // red
                } else if (percent >= 50) {
                    bar.style.backgroundColor = 'hsl(24, 90%, 55%)'; // orange
                } else {
                    bar.style.backgroundColor = 'hsl(142, 71%, 42%)'; // green
                }
            }
        }
    });
}

document.addEventListener('DOMContentLoaded', init);
