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
const pageJobs = document.getElementById('page-jobs');
const pageStats = document.getElementById('page-stats');
const monthSelect = document.getElementById('stats-month-select');

let isClientRunning = false;
let currentJobs = [];
let selectedMonth = 'current';
let updateInterval = null;

async function init() {
    await checkClientStatus();
    await fetchJobs();
    
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

    monthSelect?.addEventListener('change', (e) => {
        selectedMonth = e.target.value;
        calculateStatistics(currentJobs);
    });
}

function switchTab(tab) {
    if (tab === 'jobs') {
        tabJobs.classList.add('active');
        tabStats.classList.remove('active');
        pageJobs.classList.add('active');
        pageStats.classList.remove('active');
    } else {
        tabStats.classList.add('active');
        tabJobs.classList.remove('active');
        pageStats.classList.add('active');
        pageJobs.classList.remove('active');
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
    } catch (error) {
        console.error('Failed to toggle client:', error);
        alert(`Error: ${error}`);
    } finally {
        toggleClientBtn.disabled = false;
    }
}

async function fetchJobs() {
    try {
        const jobs = await invoke('get_jobs');
        currentJobs = jobs;
        renderJobs(jobs);
        calculateStatistics(jobs);
    } catch (error) {
        console.error('Failed to fetch jobs:', error);
    }
}

function calculateJobPages(job) {
    const copies = parseInt(job.attributes.copies, 10) || 1;
    const pageRanges = job.attributes.pageRanges || '';
    
    let pageCount = 1;
    if (pageRanges && pageRanges.trim() !== '') {
        const parts = pageRanges.split(',');
        let count = 0;
        parts.forEach(part => {
            const range = part.trim().split('-');
            if (range.length === 2) {
                const start = parseInt(range[0], 10);
                const end = parseInt(range[1], 10);
                if (!isNaN(start) && !isNaN(end) && end >= start) {
                    count += (end - start + 1);
                }
            } else if (range.length === 1) {
                const page = parseInt(range[0], 10);
                if (!isNaN(page)) {
                    count += 1;
                }
            }
        });
        if (count > 0) {
            pageCount = count;
        }
    }
    return pageCount * copies;
}

function calculateStatistics(jobs) {
    let filteredJobs = jobs;
    if (selectedMonth === 'current') {
        const now = new Date();
        const currentYearMonth = `${now.getFullYear()}-${String(now.getMonth() + 1).padStart(2, '0')}`;
        filteredJobs = jobs.filter(job => {
            if (!job.updatedAt) return true;
            return job.updatedAt.startsWith(currentYearMonth);
        });
    } else if (selectedMonth !== 'all') {
        filteredJobs = jobs.filter(job => {
            if (!job.updatedAt) return false;
            return job.updatedAt.startsWith(selectedMonth);
        });
    }

    let count1sMono = 0;
    let count2sMono = 0;
    let count1sColor = 0;
    let count2sColor = 0;

    filteredJobs.forEach(job => {
        const pages = calculateJobPages(job);
        const isColor = job.attributes.color === 'Color' || job.attributes.color === 'color';
        const isTwoSided = job.attributes.sides && job.attributes.sides.startsWith('two-sided');

        if (isColor) {
            if (isTwoSided) {
                count2sColor += pages;
            } else {
                count1sColor += pages;
            }
        } else {
            if (isTwoSided) {
                count2sMono += pages;
            } else {
                count1sMono += pages;
            }
        }
    });

    const price1sMono = count1sMono * 3;
    const price2sMono = count2sMono * 2;
    const price1sColor = count1sColor * 10;
    const price2sColor = count2sColor * 8;

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
}

document.addEventListener('DOMContentLoaded', init);
